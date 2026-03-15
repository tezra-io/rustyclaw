defmodule RustyclawOrchestrator.ToolSynthesis.Composer do
  @moduledoc """
  Tool composition layer for synthesized tools.

  Tracks dependencies between synthesized tools and provides a
  `call_tool/2` function that tools can use to invoke other synthesized
  tools. On deprecation of a dependency, dependent tools are cascade-suspended.

  Dependency tracking is stored in an ETS table for concurrent read access.
  """

  use GenServer

  require Logger

  alias RustyclawOrchestrator.ToolSynthesis.{Registry, Sandbox}

  @table :rustyclaw_synth_deps

  @doc "Initialize the ETS table. Called once at application start."
  @spec init_table() :: :ok
  def init_table do
    :ets.new(@table, [:named_table, :set, :public, read_concurrency: true])
    :ok
  end

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, _init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, [], name: name)
  end

  @doc """
  Call a synthesized tool by name with the given params.

  This is the entry point for tool-to-tool composition. Goes through
  the Sandbox for execution safety.
  """
  @spec call_tool(String.t(), map()) :: {:ok, String.t()} | {:error, String.t()}
  def call_tool(tool_name, params) when is_binary(tool_name) and is_map(params) do
    case Registry.lookup(tool_name) do
      {:ok, entry} ->
        if entry.status in [:probation, :promoted] do
          Sandbox.execute(entry.module, params)
        else
          {:error, "tool #{tool_name} is #{entry.status}, not executable"}
        end

      {:error, :not_found} ->
        {:error, "tool #{tool_name} not found"}
    end
  end

  @doc """
  Register a dependency: `dependent` depends on `dependency`.
  """
  @spec add_dependency(String.t(), String.t(), keyword()) :: :ok
  def add_dependency(dependent, dependency, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:add_dependency, dependent, dependency})
  end

  @doc """
  Remove a dependency link.
  """
  @spec remove_dependency(String.t(), String.t(), keyword()) :: :ok
  def remove_dependency(dependent, dependency, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:remove_dependency, dependent, dependency})
  end

  @doc """
  Get all dependencies for a tool.
  """
  @spec get_dependencies(String.t()) :: [String.t()]
  def get_dependencies(tool_name) when is_binary(tool_name) do
    case :ets.lookup(@table, {:deps, tool_name}) do
      [{_, deps}] -> deps
      [] -> []
    end
  end

  @doc """
  Get all tools that depend on the given tool.
  """
  @spec get_dependents(String.t()) :: [String.t()]
  def get_dependents(tool_name) when is_binary(tool_name) do
    case :ets.lookup(@table, {:rev, tool_name}) do
      [{_, dependents}] -> dependents
      [] -> []
    end
  end

  @doc """
  Handle deprecation of a tool by cascade-suspending its dependents.

  Returns the list of tools that were suspended.
  """
  @spec cascade_deprecate(String.t(), keyword()) :: {:ok, [String.t()]}
  def cascade_deprecate(tool_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:cascade_deprecate, tool_name})
  end

  @doc "Clear all dependency data. Used in tests."
  @spec clear() :: :ok
  def clear do
    :ets.delete_all_objects(@table)
    :ok
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    {:ok, %{}}
  end

  @impl true
  def handle_call({:add_dependency, dependent, dependency}, _from, state) do
    # Forward direction: dependent -> [dependencies]
    deps = get_dependencies(dependent)

    unless dependency in deps do
      :ets.insert(@table, {{:deps, dependent}, [dependency | deps]})

      # Reverse direction: dependency -> [dependents]
      rev = get_dependents(dependency)
      :ets.insert(@table, {{:rev, dependency}, [dependent | rev]})
    end

    {:reply, :ok, state}
  end

  def handle_call({:remove_dependency, dependent, dependency}, _from, state) do
    deps = get_dependencies(dependent)
    :ets.insert(@table, {{:deps, dependent}, List.delete(deps, dependency)})

    rev = get_dependents(dependency)
    :ets.insert(@table, {{:rev, dependency}, List.delete(rev, dependent)})

    {:reply, :ok, state}
  end

  def handle_call({:cascade_deprecate, tool_name}, _from, state) do
    suspended = do_cascade(tool_name, [])
    {:reply, {:ok, suspended}, state}
  end

  # --- Cascade Logic ---

  defp do_cascade(tool_name, already_suspended) do
    dependents = get_dependents(tool_name)

    Enum.reduce(dependents, already_suspended, fn dep, acc ->
      maybe_suspend_dependent(dep, tool_name, acc)
    end)
  end

  defp maybe_suspend_dependent(dep, cause, acc) do
    if dep in acc do
      acc
    else
      case Registry.lookup(dep) do
        {:ok, entry} when entry.status in [:probation, :promoted] ->
          Logger.info("Cascade-suspending tool #{dep} — dependency #{cause} deprecated")
          Registry.update_status(dep, :suspended)
          do_cascade(dep, [dep | acc])

        _ ->
          acc
      end
    end
  end
end
