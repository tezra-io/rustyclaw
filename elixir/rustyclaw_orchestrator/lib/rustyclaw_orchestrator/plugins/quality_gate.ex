defmodule RustyclawOrchestrator.Plugins.QualityGate do
  @moduledoc """
  Post-task validation via shell commands.

  Takes a result and a list of quality gate configurations, executes
  each gate as a shell command, and returns pass/fail with outputs.
  """

  require Logger

  @default_timeout_ms 120_000

  @doc """
  Run quality gates against a task result.

  Each gate is a map with:
  - `:name` — human-readable gate name (e.g., "test", "lint")
  - `:command` — shell command to execute (e.g., "cargo test", "mix credo")
  - `:timeout` — optional timeout in ms (default: 120_000)
  - `:cwd` — optional working directory

  Returns `{:pass, outputs}` if all gates pass, or `{:fail, gate_name, output}`
  on the first failure.
  """
  @spec run(term(), [map()]) :: {:pass, [map()]} | {:fail, String.t(), map()}
  def run(_result, []), do: {:pass, []}

  def run(_result, gates) when is_list(gates) do
    run_gates(gates, [])
  end

  defp run_gates([], outputs), do: {:pass, Enum.reverse(outputs)}

  defp run_gates([gate | rest], outputs) do
    name = gate[:name] || gate["name"] || "unnamed"
    command = gate[:command] || gate["command"]
    timeout = gate[:timeout] || gate["timeout"] || @default_timeout_ms
    cwd = gate[:cwd] || gate["cwd"]

    Logger.info("Running quality gate: #{name} (#{command})")

    case execute_command(command, timeout, cwd) do
      {:ok, output} ->
        result = %{name: name, status: :pass, output: output}
        run_gates(rest, [result | outputs])

      {:error, output} ->
        result = %{name: name, status: :fail, output: output}
        Logger.warning("Quality gate failed: #{name}")
        {:fail, name, result}
    end
  end

  defp execute_command(command, timeout, cwd) do
    opts = [stderr_to_stdout: true]
    opts = if cwd, do: Keyword.put(opts, :cd, cwd), else: opts

    task =
      Task.async(fn ->
        System.cmd("sh", ["-c", command], opts)
      end)

    case Task.yield(task, timeout) || Task.shutdown(task) do
      {:ok, {output, 0}} ->
        {:ok, output}

      {:ok, {output, exit_code}} ->
        {:error, "exit code #{exit_code}: #{output}"}

      nil ->
        {:error, "timeout after #{timeout}ms"}
    end
  end
end
