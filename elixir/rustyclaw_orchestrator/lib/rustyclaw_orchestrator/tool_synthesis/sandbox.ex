defmodule RustyclawOrchestrator.ToolSynthesis.Sandbox do
  @moduledoc """
  Sandboxed execution environment for synthesized tools.

  All synthesized tools execute under a dedicated Task.Supervisor.
  Each execution is wrapped in a supervised task with a hard timeout.
  Output is validated for type correctness and size.
  """

  require Logger

  @default_timeout_ms 30_000
  @max_output_bytes 1_048_576
  @supervisor __MODULE__.TaskSupervisor

  @doc """
  Returns the child spec for the sandbox's Task.Supervisor.
  """
  def child_spec(_opts) do
    %{
      id: @supervisor,
      start: {Task.Supervisor, :start_link, [[name: @supervisor]]},
      type: :supervisor
    }
  end

  @doc """
  Execute a synthesized tool module with the given params.

  Runs the tool in a supervised Task with a hard timeout.
  Validates the return type and output size.

  Options:
  - `:timeout` — execution timeout in ms (default: 30_000)
  """
  @spec execute(module(), map(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def execute(module, params, opts \\ []) when is_atom(module) and is_map(params) do
    timeout = Keyword.get(opts, :timeout, @default_timeout_ms)

    task =
      Task.Supervisor.async_nolink(@supervisor, fn ->
        module.execute(params)
      end)

    case Task.yield(task, timeout) || Task.shutdown(task, :brutal_kill) do
      {:ok, result} ->
        validate_output(result)

      {:exit, reason} ->
        {:error, "tool process crashed: #{inspect(reason)}"}

      nil ->
        {:error, "tool execution timed out after #{timeout}ms"}
    end
  end

  defp validate_output({:ok, output}) when is_binary(output) do
    if byte_size(output) > @max_output_bytes do
      truncated = binary_part(output, 0, @max_output_bytes)
      {:ok, truncated}
    else
      {:ok, output}
    end
  end

  defp validate_output({:error, reason}) when is_binary(reason) do
    {:error, reason}
  end

  defp validate_output(other) do
    {:error,
     "invalid tool output: expected {:ok, string} | {:error, string}, got #{inspect(other)}"}
  end
end
