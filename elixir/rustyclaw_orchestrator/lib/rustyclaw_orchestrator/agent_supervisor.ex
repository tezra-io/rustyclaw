defmodule RustyclawOrchestrator.AgentSupervisor do
  @moduledoc """
  DynamicSupervisor for agent processes.

  Provides spawn/stop/list operations for AgentServer instances.
  Each agent is supervised with :one_for_one strategy, max 3 restarts per 5 seconds.
  """

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer}

  @doc "Spawn a new agent from an AgentDefinition."
  @spec spawn_agent(AgentDefinition.t()) :: {:ok, pid()} | {:error, term()}
  def spawn_agent(%AgentDefinition{} = definition) do
    child_spec = {AgentServer, definition: definition}
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

  @doc "Count running agents."
  @spec count_agents() :: non_neg_integer()
  def count_agents do
    DynamicSupervisor.count_children(__MODULE__).active
  end
end
