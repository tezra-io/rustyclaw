defmodule RustyclawOrchestrator.Tools.MessageAgentTool do
  @moduledoc """
  Tool for sending messages between agents.

  Supports both synchronous task execution (run_task) and
  asynchronous messaging (send_message).

  ## Parameters

    * `target` — required, name of the target agent (string)
    * `message` — required, message content (string)
    * `mode` — `:sync` for run_task, `:async` for send_message (default: `:async`)

  ## Returns

    * `{:ok, %{delivered: true, result: term()}}` for sync mode
    * `{:ok, %{delivered: true}}` for async mode
    * `{:error, reason}` on failure
  """

  alias RustyclawOrchestrator.AgentServer

  @max_timeout_ms 300_000

  @doc "Execute the message_agent tool."
  @spec execute(map()) :: {:ok, map()} | {:error, String.t()}
  def execute(params) when is_map(params) do
    with {:ok, target} <- require_param(params, :target),
         {:ok, message} <- require_param(params, :message) do
      mode = get_param(params, :mode, :async)
      timeout_ms = get_timeout_ms(params)
      do_message(target, message, mode, timeout_ms)
    end
  end

  def execute(_), do: {:error, "params must be a map"}

  defp do_message(target, message, :sync, timeout_ms) do
    case agent_exists?(target) do
      true ->
        case AgentServer.run_task(target, message, timeout: timeout_ms) do
          {:ok, result} -> {:ok, %{delivered: true, mode: :sync, result: result}}
          {:error, reason} -> {:error, "task failed: #{inspect(reason)}"}
        end

      false ->
        {:error, "agent '#{target}' not found"}
    end
  end

  defp do_message(target, message, :async, _timeout_ms) do
    case agent_exists?(target) do
      true ->
        AgentServer.send_message(target, message)
        {:ok, %{delivered: true, mode: :async}}

      false ->
        {:error, "agent '#{target}' not found"}
    end
  end

  defp do_message(_target, _message, mode, _timeout_ms) do
    {:error, "invalid mode: #{inspect(mode)}, expected :sync or :async"}
  end

  defp get_timeout_ms(params) do
    raw = Map.get(params, "timeout_ms", Map.get(params, :timeout_ms, @max_timeout_ms))

    case raw do
      v when is_integer(v) -> max(1, min(v, @max_timeout_ms))
      _ -> @max_timeout_ms
    end
  end

  defp require_param(params, key) do
    value = Map.get(params, key, Map.get(params, to_string(key)))

    case value do
      nil -> {:error, "missing required parameter: #{key}"}
      "" -> {:error, "#{key} cannot be empty"}
      v when is_binary(v) -> {:ok, v}
      _ -> {:error, "#{key} must be a string"}
    end
  end

  defp get_param(params, key, default) do
    value = Map.get(params, key, Map.get(params, to_string(key), default))

    cond do
      is_atom(value) -> value
      is_binary(value) -> String.to_existing_atom(value)
      true -> default
    end
  rescue
    ArgumentError -> default
  end

  defp agent_exists?(name) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, name) do
      [{_pid, _}] -> true
      [] -> false
    end
  end
end
