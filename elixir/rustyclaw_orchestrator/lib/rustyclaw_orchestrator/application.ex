defmodule RustyclawOrchestrator.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # Initialize ETS table for sub-agent session tracking
    RustyclawOrchestrator.SubAgentSession.init()

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

      # Capability routing and delegation ACL
      RustyclawOrchestrator.AgentCoordinator,

      # HTTP bridge to Rust/RustyClaw core
      RustyclawOrchestrator.RustBridge
    ]

    opts = [strategy: :one_for_one, name: RustyclawOrchestrator.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
