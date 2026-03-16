defmodule RustyclawOrchestrator.Plugins.BaseLLMPlugin do
  @moduledoc """
  Shared logic for LLM-backed plugins.

  Provides SSE stream parsing, tool call extraction from LLM responses
  (both Anthropic and OpenAI formats), approximate token counting, and
  conversation history management with sliding window summarization.

  Used via `use RustyclawOrchestrator.Plugins.BaseLLMPlugin, opts`.
  """

  defmacro __using__(opts) do
    quote do
      @behaviour RustyclawOrchestrator.Plugins.Behaviour

      @base_llm_opts unquote(opts)

      import RustyclawOrchestrator.Plugins.BaseLLMPlugin,
        only: [
          parse_sse_stream: 2,
          extract_tool_calls: 1,
          count_tokens: 1,
          manage_conversation: 3
        ]
    end
  end

  @doc """
  Parse a Server-Sent Events stream, calling `event_handler` for each data line.

  Returns the accumulated data from all events as a list of decoded JSON maps.
  Non-JSON data lines are returned as raw strings.
  """
  @spec parse_sse_stream(binary(), (term() -> :ok)) :: [term()]
  def parse_sse_stream(body, event_handler) when is_binary(body) do
    body
    |> String.split("\n")
    |> Enum.reduce([], fn line, acc ->
      case line do
        "data: [DONE]" ->
          acc

        "data: " <> data ->
          parsed = decode_sse_data(data)
          event_handler.({:chunk, data})
          [parsed | acc]

        _ ->
          acc
      end
    end)
    |> Enum.reverse()
  end

  defp decode_sse_data(data) do
    case Jason.decode(data) do
      {:ok, json} -> json
      {:error, _} -> data
    end
  end

  @doc """
  Extract tool calls from an LLM response.

  Handles both Anthropic format (content blocks with type "tool_use")
  and OpenAI format (tool_calls array on message).

  Returns a list of `%{id: id, name: name, args: args}` maps.
  """
  @spec extract_tool_calls(map()) :: [map()]
  def extract_tool_calls(response) when is_map(response) do
    cond do
      # Anthropic format: content blocks with type "tool_use"
      is_list(response["content"]) ->
        response["content"]
        |> Enum.filter(&(&1["type"] == "tool_use"))
        |> Enum.map(fn block ->
          %{
            id: block["id"],
            name: block["name"],
            args: block["input"] || %{}
          }
        end)

      # OpenAI format: tool_calls on message/choices
      is_list(response["tool_calls"]) ->
        Enum.map(response["tool_calls"], fn call ->
          args = decode_tool_args(call["function"]["arguments"])

          %{
            id: call["id"],
            name: call["function"]["name"],
            args: args
          }
        end)

      # OpenAI chat completion format: choices[0].message.tool_calls
      is_list(response["choices"]) ->
        response["choices"]
        |> List.first(%{})
        |> get_in([Access.key("message", %{}), Access.key("tool_calls", [])])
        |> Kernel.||(
          response["choices"]
          |> List.first(%{})
          |> Map.get("tool_calls", [])
        )
        |> then(fn
          nil -> []
          calls when is_list(calls) -> calls
          _ -> []
        end)
        |> Enum.map(fn call ->
          func = call["function"] || %{}
          args = decode_tool_args(func["arguments"])
          %{id: call["id"], name: func["name"], args: args}
        end)

      true ->
        []
    end
  end

  defp decode_tool_args(nil), do: %{}
  defp decode_tool_args(args) when is_map(args), do: args

  defp decode_tool_args(args) when is_binary(args) do
    case Jason.decode(args) do
      {:ok, decoded} -> decoded
      {:error, _} -> %{"raw" => args}
    end
  end

  @doc """
  Approximate token count for a string or conversation.

  Uses the ~4 characters per token heuristic for English text.
  Accepts a string or a list of message maps (sums content fields).
  """
  @spec count_tokens(binary() | [map()]) :: non_neg_integer()
  def count_tokens(text) when is_binary(text) do
    # ~4 chars per token is a reasonable approximation
    max(div(String.length(text), 4), 1)
  end

  def count_tokens(messages) when is_list(messages) do
    Enum.reduce(messages, 0, fn msg, acc ->
      content = msg["content"] || msg[:content] || ""

      token_count =
        case content do
          c when is_binary(c) -> count_tokens(c)
          blocks when is_list(blocks) -> count_tokens(Jason.encode!(blocks))
          _ -> 0
        end

      acc + token_count
    end)
  end

  @doc """
  Manage conversation history with sliding window.

  When total tokens exceed `max_tokens * 0.9`, summarizes middle turns
  to keep the conversation within the context window. Keeps the system
  prompt (first message), the most recent user message, and the last 2
  assistant turns.

  Returns `{updated_messages, total_tokens}`.
  """
  @spec manage_conversation([map()], non_neg_integer(), [map()]) :: {[map()], non_neg_integer()}
  def manage_conversation(messages, max_tokens, tool_results \\ [])

  def manage_conversation(messages, max_tokens, tool_results) do
    all_messages = messages ++ tool_results
    total = count_tokens(all_messages)
    threshold = trunc(max_tokens * 0.9)

    if total > threshold and length(all_messages) > 4 do
      summarize_middle(all_messages, total)
    else
      {all_messages, total}
    end
  end

  defp summarize_middle(messages, _total_tokens) do
    # Keep: first message (system), last 3 messages (recent context)
    {head, rest} = Enum.split(messages, 1)
    {middle, tail} = Enum.split(rest, max(length(rest) - 3, 0))

    summary_text =
      Enum.map_join(middle, "\n", fn msg ->
        role = msg["role"] || msg[:role] || "unknown"
        content = msg["content"] || msg[:content] || ""
        content_str = if is_binary(content), do: content, else: Jason.encode!(content)
        "#{role}: #{String.slice(content_str, 0, 200)}"
      end)

    summary_msg = %{
      "role" => "system",
      "content" => "[Conversation summary: #{length(middle)} messages condensed]\n#{summary_text}"
    }

    summarized = head ++ [summary_msg] ++ tail
    {summarized, count_tokens(summarized)}
  end
end
