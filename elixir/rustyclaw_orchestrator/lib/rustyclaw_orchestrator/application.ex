defmodule RustyclawOrchestrator.Application do
  @moduledoc false

  use Application

  alias RustyclawOrchestrator.ToolSynthesis

  @impl true
  def start(_type, _args) do
    # Initialize ETS tables
    RustyclawOrchestrator.SubAgentSession.init()
    RustyclawOrchestrator.ResourceLock.init()
    ToolSynthesis.Registry.init()
    ToolSynthesis.Composer.init_table()

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

      # Tool probation lifecycle state machine
      RustyclawOrchestrator.ToolSynthesis.Probation,

      # Tool composition and dependency tracking
      RustyclawOrchestrator.ToolSynthesis.Composer,

      # Iterative tool improvement with versioning
      RustyclawOrchestrator.ToolSynthesis.Improver,

      # --- Plugin subsystem ---

      # Progress tracking, loop detection, stuck worker detection
      RustyclawOrchestrator.Plugins.ProgressTracker,

      # Plugin pool management, dispatch, rate limits, capability routing
      RustyclawOrchestrator.Plugins.Manager,

      # Retry scheduling with exponential backoff and fallback routing
      RustyclawOrchestrator.Plugins.RetryScheduler,

      # Dev session orchestration — issue iteration, quality gates, progress
      RustyclawOrchestrator.Plugins.TaskOrchestrator,

      # Priority queue fed from Linear with auto-assignment
      {RustyclawOrchestrator.Plugins.TaskQueue, poll_interval_ms: 0},

      # Task supervisor for plugin Worker task dispatch
      {Task.Supervisor, name: RustyclawOrchestrator.Plugins.TaskSupervisor},

      # Dynamic supervisor for plugin Worker processes
      {DynamicSupervisor,
       name: RustyclawOrchestrator.Plugins.WorkerSupervisor,
       strategy: :one_for_one,
       max_restarts: 5,
       max_seconds: 5},

      # HTTP bridge to Rust/RustyClaw core
      {RustyclawOrchestrator.RustBridge,
       Application.get_env(:rustyclaw_orchestrator, :rust_bridge, [])},

      # HTTP API for tool synthesis (Bandit + Plug) — loopback only
      {Bandit,
       plug: RustyclawOrchestrator.ToolSynthesis.ApiRouter,
       port: Application.get_env(:rustyclaw_orchestrator, :synth_api_port, 4001),
       scheme: :http,
       ip: {127, 0, 0, 1}},

      # HTTP API for plugin management and task execution
      Supervisor.child_spec(
        {Bandit,
         plug: RustyclawOrchestrator.Plugins.PluginRouter,
         port: Application.get_env(:rustyclaw_orchestrator, :plugin_api_port, 4002),
         scheme: :http},
        id: :plugin_api_bandit
      )
    ]

    opts = [strategy: :one_for_one, name: RustyclawOrchestrator.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
