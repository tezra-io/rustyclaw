defmodule ZeroclawOrchestrator.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # Initialize ETS table for sub-agent session tracking
    ZeroclawOrchestrator.SubAgentSession.init()

    children = [
      # Agent name -> pid mapping (:unique mode)
      {Registry, keys: :unique, name: ZeroclawOrchestrator.AgentRegistry},

      # Dynamic agent lifecycle management — max 3 restarts per 5 seconds
      {DynamicSupervisor,
       name: ZeroclawOrchestrator.AgentSupervisor,
       strategy: :one_for_one,
       max_restarts: 3,
       max_seconds: 5}

      # Future children (TEZ-145+):
      # - AgentCoordinator (capability routing, delegation ACL)
      # - RustBridge (HTTP/Port connection to Rust core)
    ]

    opts = [strategy: :one_for_one, name: ZeroclawOrchestrator.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
