defmodule RustyclawOrchestrator.Tools.ListAgentsTool do
  @moduledoc """
  Tool for listing running agents with optional filtering.

  Returns either a simple name list or detailed agent information
  depending on the `detailed` parameter.

  ## Parameters

    * `detailed` — boolean, return full agent state info (default: false)
    * `capability` — filter to agents with this capability (optional)
    * `status` — filter to agents with this status (optional)

  ## Returns

    * `{:ok, %{agents: list()}}` — list of agent names or detailed maps
  """

  alias RustyclawOrchestrator.{AgentServer, AgentSupervisor}

  @doc "Execute the list_agents tool."
  @spec execute(map()) :: {:ok, map()}
  def execute(params \\ %{}) do
    detailed = Map.get(params, :detailed, Map.get(params, "detailed", false))
    capability = Map.get(params, :capability, Map.get(params, "capability"))
    status_filter = Map.get(params, :status, Map.get(params, "status"))

    agents =
      if detailed do
        AgentSupervisor.list_agents_detailed()
      else
        build_simple_list()
      end

    agents =
      agents
      |> maybe_filter_capability(capability)
      |> maybe_filter_status(status_filter)

    {:ok, %{agents: agents, count: length(agents)}}
  end

  defp build_simple_list do
    AgentSupervisor.list_agents()
    |> Enum.map(fn name ->
      case safe_get_state(name) do
        {:ok, state} ->
          %{
            name: name,
            status: state.status,
            health: state.health,
            capabilities: state.definition.capabilities
          }

        :error ->
          %{name: name, status: :unknown, health: :unknown, capabilities: []}
      end
    end)
  end

  defp maybe_filter_capability(agents, nil), do: agents

  defp maybe_filter_capability(agents, capability) when is_binary(capability) do
    Enum.filter(agents, fn agent ->
      caps = Map.get(agent, :capabilities, [])
      capability in caps
    end)
  end

  defp maybe_filter_status(agents, nil), do: agents

  defp maybe_filter_status(agents, status) do
    status_atom = if is_binary(status), do: String.to_existing_atom(status), else: status

    Enum.filter(agents, fn agent ->
      Map.get(agent, :status) == status_atom
    end)
  rescue
    ArgumentError -> agents
  end

  defp safe_get_state(agent_name) do
    {:ok, AgentServer.get_state(agent_name)}
  catch
    :exit, _ -> :error
  end
end
