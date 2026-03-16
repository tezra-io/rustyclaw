defmodule RustyclawOrchestrator.Plugins.ClaudeCodePlugin do
  @moduledoc """
  Plugin adapter for the Anthropic Messages API.

  Uses BaseLLMPlugin for shared SSE parsing, tool call extraction,
  and conversation history management. Implements the full Behaviour
  contract for integration with the plugin system.
  """

  use RustyclawOrchestrator.Plugins.BaseLLMPlugin,
    api_base: "https://api.anthropic.com/v1/messages",
    auth_header: "x-api-key"

  require Logger

  @anthropic_version "2023-06-01"

  @impl true
  def connect(config) do
    api_key = config[:api_key] || config["api_key"] || System.get_env("ANTHROPIC_API_KEY")

    if is_nil(api_key) or api_key == "" do
      {:error, :missing_api_key}
    else
      state = %{
        api_key: api_key,
        model: config[:model] || config["model"] || "claude-sonnet-4-5-20250514",
        max_tokens: config[:max_tokens] || config["max_tokens"] || 16_384,
        api_base: @base_llm_opts[:api_base],
        messages: [],
        last_rate_limit: %{remaining: nil, reset_at: nil, limited: false}
      }

      {:ok, state}
    end
  end

  @impl true
  def execute(state, task, event_handler) do
    messages = build_messages(state, task)

    body =
      Jason.encode!(%{
        model: state.model,
        max_tokens: state.max_tokens,
        messages: messages
      })

    headers = [
      {@base_llm_opts[:auth_header], state.api_key},
      {"content-type", "application/json"},
      {"anthropic-version", @anthropic_version}
    ]

    case make_request(state.api_base, headers, body) do
      {:ok, %{status: 200, body: resp_body, headers: resp_headers}} ->
        handle_success(state, messages, resp_body, resp_headers, event_handler)

      {:ok, %{status: 429, headers: resp_headers}} ->
        {:error, {:rate_limited, get_retry_after(resp_headers)}}

      {:ok, %{status: status, body: resp_body}} ->
        Logger.warning("Anthropic API error: status=#{status}")
        {:error, {:api_error, status, resp_body}}

      {:error, reason} ->
        {:error, {:request_failed, reason}}
    end
  end

  @impl true
  def health(state) do
    if state.api_key && state.api_key != "" do
      case state.last_rate_limit do
        %{limited: true} -> :degraded
        _ -> :healthy
      end
    else
      :unhealthy
    end
  end

  @impl true
  def capabilities, do: [:coding, :analysis, :review]

  @impl true
  def rate_limit_status(state) do
    state.last_rate_limit
  end

  @impl true
  def disconnect(_state), do: :ok

  # --- Private Helpers ---

  defp handle_success(state, messages, resp_body, resp_headers, event_handler) do
    events = parse_sse_stream(resp_body, event_handler)
    response = merge_sse_events(events)
    tool_calls = extract_tool_calls(response)
    rate_limit = parse_rate_limit_headers(resp_headers)

    new_state = %{
      state
      | messages: messages ++ [%{"role" => "assistant", "content" => response["content"]}],
        last_rate_limit: rate_limit
    }

    build_result(tool_calls, response, new_state, event_handler)
  end

  defp build_result([], response, new_state, event_handler) do
    text = extract_text_content(response)
    event_handler.({:artifact, :response, text})
    {:ok, {:complete, %{output: text, response: response}}, new_state}
  end

  defp build_result(tool_calls, _response, new_state, event_handler) do
    Enum.each(tool_calls, fn tc ->
      event_handler.({:tool_use, tc.name, tc.args})
    end)

    {:ok, {:tool_use, tool_calls}, new_state}
  end

  defp build_messages(state, task) do
    description = Map.get(task, :description) || Map.get(task, "description") || ""
    context = Map.get(task, :context) || %{}
    tool_results = Map.get(task, :tool_results)
    system_content = extract_system_content(context)

    base_messages = base_messages_for(state, description, system_content)
    append_tool_results(base_messages, tool_results)
  end

  defp extract_system_content(%{claude_md: md}) when is_binary(md), do: md
  defp extract_system_content(_), do: ""

  defp base_messages_for(%{messages: msgs}, _desc, _sys) when msgs != [],
    do: msgs

  defp base_messages_for(_state, description, system_content),
    do: [%{"role" => "user", "content" => build_user_content(description, system_content)}]

  defp append_tool_results(messages, results) when is_list(results) and results != [] do
    tool_result_content =
      Enum.map(results, fn tr ->
        %{
          "type" => "tool_result",
          "tool_use_id" => tr[:id] || tr["id"] || "unknown",
          "content" => inspect(tr[:result] || tr["result"] || "no result")
        }
      end)

    messages ++ [%{"role" => "user", "content" => tool_result_content}]
  end

  defp append_tool_results(messages, _), do: messages

  defp build_user_content(description, "") do
    description
  end

  defp build_user_content(description, system_content) do
    "#{system_content}\n\n---\n\n#{description}"
  end

  defp make_request(url, headers, body) do
    Req.post(url, headers: headers, body: body, receive_timeout: 300_000, decode_body: false)
  end

  defp merge_sse_events(events) do
    # If SSE parsing returned decoded JSON objects, take the last complete one.
    # If the response was not SSE (direct JSON), events is a list with one item.
    case events do
      [] -> %{}
      list -> List.last(list) |> ensure_map()
    end
  end

  defp ensure_map(val) when is_map(val), do: val

  defp ensure_map(val) when is_binary(val) do
    case Jason.decode(val) do
      {:ok, map} when is_map(map) -> map
      _ -> %{"content" => [%{"type" => "text", "text" => val}]}
    end
  end

  defp ensure_map(_), do: %{}

  defp extract_text_content(%{"content" => blocks}) when is_list(blocks) do
    blocks
    |> Enum.filter(&(&1["type"] == "text"))
    |> Enum.map_join("\n", & &1["text"])
  end

  defp extract_text_content(_), do: ""

  defp parse_rate_limit_headers(headers) do
    headers_map = normalize_headers(headers)

    remaining =
      case headers_map["x-ratelimit-remaining-requests"] do
        nil -> nil
        val -> parse_integer(val)
      end

    reset_at =
      case headers_map["x-ratelimit-reset-requests"] do
        nil ->
          nil

        val ->
          case DateTime.from_iso8601(to_string(val)) do
            {:ok, dt, _} -> dt
            _ -> nil
          end
      end

    limited = remaining != nil and remaining == 0

    %{remaining: remaining, reset_at: reset_at, limited: limited}
  end

  defp get_retry_after(headers) do
    headers_map = normalize_headers(headers)

    case headers_map["retry-after"] do
      nil -> 60
      val -> parse_integer(val) || 60
    end
  end

  # Req 0.5+ returns headers as %{String.t() => [String.t()]}
  defp normalize_headers(headers) when is_map(headers) do
    Map.new(headers, fn {k, v} ->
      val =
        case v do
          [first | _] -> first
          other -> to_string(other)
        end

      {String.downcase(to_string(k)), val}
    end)
  end

  defp normalize_headers(headers) when is_list(headers) do
    Enum.reduce(headers, %{}, fn {k, v}, acc ->
      Map.put(acc, String.downcase(to_string(k)), to_string(v))
    end)
  end

  defp parse_integer(val) when is_integer(val), do: val

  defp parse_integer(val) when is_binary(val) do
    case Integer.parse(val) do
      {n, _} -> n
      :error -> nil
    end
  end

  defp parse_integer(_), do: nil
end
