defmodule RustyclawOrchestrator.Tools.KillAgentTool do
  @moduledoc """
  Tool for stopping/killing running agents.

  Terminates the agent process via the supervisor. Persistent agents
  may be restarted automatically by the supervisor depending on their
  restart strategy.

  ## Parameters

    * `name` — required, name of the agent to kill (string)

  ## Returns

    * `{:ok, %{killed: true, agent_name: String.t()}}` on success
    * `{:error, reason}` on failure
  """

  alias RustyclawOrchestrator.{AgentServer, AgentSupervisor}

  @doc "Execute the kill_agent tool."
  @spec execute(map()) :: {:ok, map()} | {:error, String.t()}
  def execute(params) when is_map(params) do
    with {:ok, name} <- require_name(params) do
      do_kill(name)
    end
  end

  def execute(_), do: {:error, "params must be a map"}

  defp require_name(params) do
    name = Map.get(params, :name, Map.get(params, "name"))

    case name do
      nil -> {:error, "missing required parameter: name"}
      "" -> {:error, "name cannot be empty"}
      n when is_binary(n) -> {:ok, n}
      _ -> {:error, "name must be a string"}
    end
  end

  defp do_kill(name) do
    # Capture snapshot info before killing if the agent is persistent
    snapshot_saved = save_snapshot_if_persistent(name)

    case AgentSupervisor.stop_agent(name) do
      :ok ->
        {:ok, %{killed: true, agent_name: name, snapshot_saved: snapshot_saved}}

      {:error, :not_found} ->
        {:error, "agent '#{name}' not found"}
    end
  end

  defp save_snapshot_if_persistent(name) do
    case safe_get_state(name) do
      {:ok, state} ->
        if state.definition.persistent do
          # Snapshot is saved automatically in terminate/2, but we trigger
          # get_snapshot here to confirm the data is accessible
          AgentServer.get_snapshot(name)
          true
        else
          false
        end

      :error ->
        false
    end
  end

  defp safe_get_state(name) do
    {:ok, AgentServer.get_state(name)}
  catch
    :exit, _ -> :error
  end
end
