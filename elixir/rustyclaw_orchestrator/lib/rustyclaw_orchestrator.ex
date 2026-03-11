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

  ## Tools (TEZ-165)

  - `Tools.SpawnAgentTool` — dynamically spawn sub-agents at runtime
  - `Tools.ListAgentsTool` — list running agents with filtering
  - `Tools.MessageAgentTool` — send messages between agents
  - `Tools.KillAgentTool` — stop/kill running agents
  """

  alias RustyclawOrchestrator.{
    AgentCoordinator,
    AgentDefinition,
    AgentSupervisor,
    SubAgentSession
  }

  alias RustyclawOrchestrator.Tools.{
    KillAgentTool,
    ListAgentsTool,
    MessageAgentTool,
    SpawnAgentTool
  }

  @doc "Spawn an agent from a definition file."
  @spec spawn_from_file(Path.t()) :: {:ok, pid()} | {:error, term()}
  def spawn_from_file(path) do
    with {:ok, definition} <- AgentDefinition.from_file(path),
         {:ok, []} <- AgentDefinition.validate(definition) do
      AgentSupervisor.spawn_agent(definition)
    end
  end

  @doc "Spawn an agent dynamically from a params map (tool interface)."
  @spec spawn_agent(map()) :: {:ok, map()} | {:error, String.t()}
  defdelegate spawn_agent(params), to: SpawnAgentTool, as: :execute

  @doc "List running agents with optional filtering (tool interface)."
  @spec list_agents_tool(map()) :: {:ok, map()}
  defdelegate list_agents_tool(params \\ %{}), to: ListAgentsTool, as: :execute

  @doc "Send a message to an agent (tool interface)."
  @spec message_agent(map()) :: {:ok, map()} | {:error, String.t()}
  defdelegate message_agent(params), to: MessageAgentTool, as: :execute

  @doc "Kill a running agent (tool interface)."
  @spec kill_agent(map()) :: {:ok, map()} | {:error, String.t()}
  defdelegate kill_agent(params), to: KillAgentTool, as: :execute

  @doc "Delegate a task to the best matching agent."
  defdelegate delegate(task, opts \\ []), to: AgentCoordinator

  @doc "List running agent names."
  defdelegate list_agents(), to: AgentSupervisor

  @doc "List running agents with detailed state information."
  defdelegate list_agents_detailed(), to: AgentSupervisor

  @doc "Stop an agent by name."
  defdelegate stop_agent(name), to: AgentSupervisor

  @doc "Create a new task session."
  defdelegate create_session(agent_name, task, opts \\ []), to: SubAgentSession, as: :create
end
