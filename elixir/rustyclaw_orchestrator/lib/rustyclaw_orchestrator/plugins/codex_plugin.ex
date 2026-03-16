defmodule RustyclawOrchestrator.Plugins.CodexPlugin do
  @moduledoc """
  Plugin adapter for the OpenAI API.

  Uses BaseLLMPlugin for shared SSE parsing, tool call extraction,
  and conversation history management. Implements the full Behaviour
  contract for integration with the plugin system.
  """

  use RustyclawOrchestrator.Plugins.BaseLLMPlugin,
    api_base: "https://api.openai.com/v1/responses",
    auth_header: "Authorization"

  require Logger

  @impl true
  def connect(config) do
    api_key = config[:api_key] || config["api_key"] || System.get_env("OPENAI_API_KEY")

    if is_nil(api_key) or api_key == "" do
      {:error, :missing_api_key}
    else
      state = %{
        api_key: api_key,
        model: config[:model] || config["model"] || "codex-mini-latest",
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
      {@base_llm_opts[:auth_header], "Bearer #{state.api_key}"},
      {"content-type", "application/json"}
    ]

    case make_request(state.api_base, headers, body) do
      {:ok, %{status: 200, body: resp_body, headers: resp_headers}} ->
        handle_success(state, messages, resp_body, resp_headers, event_handler)

      {:ok, %{status: 429, headers: resp_headers}} ->
        {:error, {:rate_limited, get_retry_after(resp_headers)}}

      {:ok, %{status: status, body: resp_body}} ->
        Logger.warning("OpenAI API error: status=#{status}")
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
  def capabilities, do: [:coding]

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
    assistant_msg = extract_assistant_message(response)

    new_state = %{
      state
      | messages: messages ++ [assistant_msg],
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
  defp extract_system_content(_), do: nil

  defp base_messages_for(%{messages: msgs}, _desc, _sys) when msgs != [],
    do: msgs

  defp base_messages_for(_state, description, nil),
    do: [%{"role" => "user", "content" => description}]

  defp base_messages_for(_state, description, system_content) do
    [
      %{"role" => "system", "content" => system_content},
      %{"role" => "user", "content" => description}
    ]
  end

  defp append_tool_results(messages, results) when is_list(results) and results != [] do
    tool_msgs =
      Enum.map(results, fn tr ->
        %{
          "role" => "tool",
          "tool_call_id" => tr[:id] || tr["id"] || "unknown",
          "content" => inspect(tr[:result] || tr["result"] || "no result")
        }
      end)

    messages ++ tool_msgs
  end

  defp append_tool_results(messages, _), do: messages

  defp make_request(url, headers, body) do
    Req.post(url, headers: headers, body: body, receive_timeout: 300_000, decode_body: false)
  end

  defp merge_sse_events(events) do
    case events do
      [] -> %{}
      list -> List.last(list) |> ensure_map()
    end
  end

  defp ensure_map(val) when is_map(val), do: val

  defp ensure_map(val) when is_binary(val) do
    case Jason.decode(val) do
      {:ok, map} when is_map(map) -> map
      _ -> %{"choices" => [%{"message" => %{"content" => val}}]}
    end
  end

  defp ensure_map(_), do: %{}

  defp extract_assistant_message(%{"choices" => [choice | _]}) do
    message = choice["message"] || %{}
    %{"role" => "assistant", "content" => message["content"] || ""}
  end

  defp extract_assistant_message(response) do
    content = response["content"] || response["output"] || ""
    %{"role" => "assistant", "content" => content}
  end

  defp extract_text_content(%{"choices" => [choice | _]}) do
    get_in(choice, ["message", "content"]) || ""
  end

  defp extract_text_content(%{"content" => content}) when is_binary(content), do: content
  defp extract_text_content(%{"output" => output}) when is_binary(output), do: output
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
