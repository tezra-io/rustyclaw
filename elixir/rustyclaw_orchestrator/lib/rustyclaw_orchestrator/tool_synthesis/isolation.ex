defmodule RustyclawOrchestrator.ToolSynthesis.Isolation do
  @moduledoc """
  Sacrificial node foundation for isolated tool execution.

  Optionally compiles and executes synthesized tools on a separate BEAM
  node for maximum crash and resource isolation. Uses `:peer` (OTP 25+)
  to start a linked node that is automatically cleaned up.

  When `tool_synthesis.use_isolated_node` is false (default), falls back
  to the existing Sandbox execution path. This module is a foundation —
  full OS-level isolation (cgroups, seccomp) is future work.
  """

  require Logger

  alias RustyclawOrchestrator.ToolSynthesis.Sandbox

  @default_timeout_ms 30_000

  @doc """
  Check whether isolated execution is enabled via config.
  """
  @spec enabled?() :: boolean()
  def enabled? do
    Application.get_env(:rustyclaw_orchestrator, :use_isolated_node, false)
  end

  @doc """
  Start an isolated peer node for tool execution.

  Returns `{:ok, pid}` where pid is the peer process, or `{:error, reason}`.
  The node is started with a minimal environment.
  """
  @spec start_node() :: {:ok, pid(), node()} | {:error, term()}
  def start_node do
    case :peer.start(%{name: peer_name(), connection: :standard}) do
      {:ok, pid, node} ->
        Logger.info("Started isolated synthesis node: #{node}")
        {:ok, pid, node}

      {:error, reason} ->
        Logger.warning("Failed to start isolated node: #{inspect(reason)}")
        {:error, reason}
    end
  end

  @doc """
  Stop an isolated peer node.
  """
  @spec stop_node(pid()) :: :ok
  def stop_node(pid) do
    :peer.stop(pid)
  end

  @doc """
  Compile source code on the isolated node.

  Sends the source string to the remote node and compiles it there.
  Returns `{:ok, module}` or `{:error, reason}`.
  """
  @spec compile_on_node(node(), String.t(), keyword()) :: {:ok, module()} | {:error, term()}
  def compile_on_node(node, source, opts \\ []) do
    timeout = Keyword.get(opts, :timeout, @default_timeout_ms)

    try do
      case :erpc.call(node, Code, :compile_string, [source], timeout) do
        [{module, _bytecode}] -> {:ok, module}
        _ -> {:error, :compilation_failed}
      end
    catch
      :exit, {:erpc, reason} -> {:error, {:node_error, reason}}
    end
  end

  @doc """
  Execute a tool module on the isolated node.

  Calls `module.execute(params)` on the remote node via `:erpc`.
  Returns `{:ok, output}` or `{:error, reason}`.
  """
  @spec execute_on_node(node(), module(), map(), keyword()) ::
          {:ok, String.t()} | {:error, String.t()}
  def execute_on_node(node, module, params, opts \\ []) do
    timeout = Keyword.get(opts, :timeout, @default_timeout_ms)

    try do
      case :erpc.call(node, module, :execute, [params], timeout) do
        {:ok, output} when is_binary(output) -> {:ok, output}
        {:error, reason} when is_binary(reason) -> {:error, reason}
        other -> {:error, "unexpected result: #{inspect(other)}"}
      end
    catch
      :exit, {:erpc, :noconnection} ->
        {:error, "isolated node disconnected"}

      :exit, {:erpc, {:exception, error, _stack}} ->
        {:error, "tool crashed on isolated node: #{inspect(error)}"}

      :exit, {:erpc, reason} ->
        {:error, "isolated node error: #{inspect(reason)}"}
    end
  end

  @doc """
  Execute a tool, using isolated node if enabled, otherwise Sandbox.

  This is the main entry point that respects the config flag.
  """
  @spec execute(module(), map(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def execute(module, params, opts \\ []) do
    if enabled?() do
      execute_isolated(module, params, opts)
    else
      Sandbox.execute(module, params, opts)
    end
  end

  defp execute_isolated(module, params, opts) do
    case start_node() do
      {:ok, pid, node} ->
        try do
          # Ensure the module is available on the remote node by loading it
          load_module_on_node(node, module)
          execute_on_node(node, module, params, opts)
        after
          stop_node(pid)
        end

      {:error, reason} ->
        Logger.warning("Isolated node failed, falling back to Sandbox: #{inspect(reason)}")
        Sandbox.execute(module, params, opts)
    end
  end

  defp load_module_on_node(node, module) do
    {_mod, binary, filename} = :code.get_object_code(module)
    :erpc.call(node, :code, :load_binary, [module, filename, binary])
  end

  defp peer_name do
    suffix = :erlang.unique_integer([:positive])
    :"rustyclaw_synth_sandbox_#{suffix}"
  end
end
