defmodule RustyclawOrchestrator.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # Initialize ETS tables
    RustyclawOrchestrator.SubAgentSession.init()
    RustyclawOrchestrator.ResourceLock.init()
    RustyclawOrchestrator.ToolSynthesis.Registry.init()

    children = [
      # Agent name -> pid mapping (:unique mode)
      {Registry, keys: :unique, name: RustyclawOrchestrator.AgentRegistry},

      # Message provenance trace storage
      RustyclawOrchestrator.TraceStore,

      # Dynamic agent lifecycle management — max 3 restarts per 5 seconds
      {DynamicSupervisor,
       name: RustyclawOrchestrator.AgentSupervisor,
       strategy: :one_for_one,
       max_restarts: 3,
       max_seconds: 5},

      # BTW side-channel task supervisor — temporary processes, high restart tolerance
      {DynamicSupervisor,
       name: RustyclawOrchestrator.BtwSupervisor,
       strategy: :one_for_one,
       max_restarts: 10,
       max_seconds: 5},

      # Task supervisor for async AgentServer delegation and health tasks
      {Task.Supervisor, name: RustyclawOrchestrator.AgentServer.TaskSupervisor},

      # Task supervisor for async AgentCoordinator delegation execution
      {Task.Supervisor, name: RustyclawOrchestrator.AgentCoordinator.TaskSupervisor},

      # Capability routing and delegation ACL
      RustyclawOrchestrator.AgentCoordinator,

      # Task supervisor for async RustBridge HTTP calls
      {Task.Supervisor, name: RustyclawOrchestrator.RustBridge.TaskSupervisor},

      # Task supervisor for sandboxed synthesized tool execution
      RustyclawOrchestrator.ToolSynthesis.Sandbox,

      # Tool synthesis engine — loads persisted tools on startup
      RustyclawOrchestrator.ToolSynthesis.Synthesizer,

      # HTTP bridge to Rust/RustyClaw core
      {RustyclawOrchestrator.RustBridge,
       Application.get_env(:rustyclaw_orchestrator, :rust_bridge, [])}
    ]

    opts = [strategy: :one_for_one, name: RustyclawOrchestrator.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
