defmodule RustyclawOrchestrator.AgentSupervisor do
  @moduledoc """
  DynamicSupervisor for agent processes.

  Provides spawn/stop/list operations for AgentServer instances.
  Each agent is supervised with :one_for_one strategy, max 3 restarts per 5 seconds.

  Persistent agents use `:permanent` restart strategy (always restarted).
  Non-persistent agents use `:temporary` restart (never restarted).
  """

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer}

  @doc """
  Spawn a new agent from an AgentDefinition.
  Accepts optional keyword opts:
  - `:parent_pid` — pid of the parent agent (for child tracking)
  """
  @spec spawn_agent(AgentDefinition.t(), keyword()) :: {:ok, pid()} | {:error, term()}
  def spawn_agent(%AgentDefinition{} = definition, opts \\ []) do
    restart = if definition.persistent, do: :permanent, else: :temporary
    parent_pid = Keyword.get(opts, :parent_pid)

    child_spec =
      %{
        id: AgentServer,
        start: {AgentServer, :start_link, [[definition: definition, parent_pid: parent_pid]]},
        restart: restart
      }

    DynamicSupervisor.start_child(__MODULE__, child_spec)
  end

  @doc "Stop an agent by name."
  @spec stop_agent(String.t()) :: :ok | {:error, :not_found}
  def stop_agent(agent_name) when is_binary(agent_name) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, agent_name) do
      [{pid, _}] ->
        DynamicSupervisor.terminate_child(__MODULE__, pid)

      [] ->
        {:error, :not_found}
    end
  end

  @doc "List all running agent names."
  @spec list_agents() :: [String.t()]
  def list_agents do
    __MODULE__
    |> DynamicSupervisor.which_children()
    |> Enum.flat_map(fn {_, pid, _, _} ->
      case Registry.keys(RustyclawOrchestrator.AgentRegistry, pid) do
        [name] -> [name]
        _ -> []
      end
    end)
  end

  @doc """
  List all running agents with detailed state information.

  Returns a list of maps with: name, status, health, persistent, parent,
  child_count, last_active_at, accumulated_state_keys, uptime_seconds.
  """
  @spec list_agents_detailed() :: [map()]
  def list_agents_detailed do
    __MODULE__
    |> DynamicSupervisor.which_children()
    |> Enum.flat_map(fn {_, pid, _, _} ->
      case Registry.keys(RustyclawOrchestrator.AgentRegistry, pid) do
        [name] -> build_agent_detail(name, pid)
        _ -> []
      end
    end)
  end

  defp build_agent_detail(name, pid) do
    case safe_get_state(name) do
      {:ok, agent_state} ->
        [
          %{
            name: name,
            pid: pid,
            status: agent_state.status,
            health: agent_state.health,
            persistent: agent_state.definition.persistent,
            parent: agent_state.definition.parent,
            parent_pid: agent_state.parent_pid,
            child_count: length(agent_state.child_pids),
            capabilities: agent_state.definition.capabilities,
            last_active_at: agent_state.last_active_at,
            accumulated_state_keys: Map.keys(agent_state.accumulated_state),
            uptime_seconds: uptime_seconds(agent_state.started_at)
          }
        ]

      :error ->
        []
    end
  end

  @doc "Count running agents."
  @spec count_agents() :: non_neg_integer()
  def count_agents do
    DynamicSupervisor.count_children(__MODULE__).active
  end

  defp safe_get_state(agent_name) do
    {:ok, AgentServer.get_state(agent_name)}
  catch
    :exit, _ -> :error
  end

  defp uptime_seconds(started_at) do
    DateTime.diff(DateTime.utc_now(), started_at, :second)
  end
end
