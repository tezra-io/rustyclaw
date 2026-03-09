defmodule RustyclawOrchestrator do
  @moduledoc """
  Elixir/OTP orchestration layer for RustyClaw.

  Manages agent lifecycle, inter-agent messaging, capability-based routing,
  and delegation on top of the Rust core (channels, tools, providers, security).

  ## Subsystems

  - `AgentDefinition` — YAML frontmatter + markdown parser for agent definitions
  - `AgentServer` — GenServer per agent (health checks, task execution, messaging)
  - `AgentSupervisor` — DynamicSupervisor for agent processes (spawn/stop/list)
  - `AgentCoordinator` — Capability-based routing and delegation ACL
  - `SubAgentSession` — ETS-backed session lifecycle tracking
  - `RustBridge` — HTTP bridge to the Rust/RustyClaw core
  """

  alias RustyclawOrchestrator.{AgentCoordinator, AgentDefinition, AgentSupervisor, SubAgentSession}

  @doc "Spawn an agent from a definition file."
  @spec spawn_from_file(Path.t()) :: {:ok, pid()} | {:error, term()}
  def spawn_from_file(path) do
    with {:ok, definition} <- AgentDefinition.from_file(path),
         {:ok, []} <- AgentDefinition.validate(definition) do
      AgentSupervisor.spawn_agent(definition)
    end
  end

  @doc "Delegate a task to the best matching agent."
  defdelegate delegate(task, opts \\ []), to: AgentCoordinator

  @doc "List running agents."
  defdelegate list_agents(), to: AgentSupervisor

  @doc "Stop an agent by name."
  defdelegate stop_agent(name), to: AgentSupervisor

  @doc "Create a new task session."
  defdelegate create_session(agent_name, task, opts \\ []), to: SubAgentSession, as: :create
end
