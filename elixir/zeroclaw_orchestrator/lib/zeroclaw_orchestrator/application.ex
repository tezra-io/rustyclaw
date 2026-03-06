defmodule ZeroclawOrchestrator.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      # Agent name -> pid mapping (:unique mode)
      {Registry, keys: :unique, name: ZeroclawOrchestrator.AgentRegistry},

      # Dynamic agent lifecycle management
      {DynamicSupervisor,
       name: ZeroclawOrchestrator.AgentSupervisor, strategy: :one_for_one}

      # Future children (TEZ-143+):
      # - AgentCoordinator (capability routing, delegation ACL)
      # - RustBridge (HTTP/Port connection to Rust core)
    ]

    opts = [strategy: :one_for_one, name: ZeroclawOrchestrator.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
