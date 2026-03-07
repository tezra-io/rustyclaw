defmodule ZeroclawOrchestrator.AgentServerTest do
  use ExUnit.Case

  alias ZeroclawOrchestrator.{AgentDefinition, AgentServer, AgentSupervisor}

  setup do
    # Clean up any agents after each test
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  defp make_definition(name) do
    %AgentDefinition{
      name: name,
      capabilities: ["test"],
      personality: "Test agent"
    }
  end

  describe "spawn and lifecycle" do
    test "spawns an agent via supervisor" do
      assert {:ok, pid} = AgentSupervisor.spawn_agent(make_definition("test-agent"))
      assert Process.alive?(pid)
    end

    test "agent is registered by name" do
      {:ok, _pid} = AgentSupervisor.spawn_agent(make_definition("named-agent"))

      assert [{pid, _}] = Registry.lookup(ZeroclawOrchestrator.AgentRegistry, "named-agent")
      assert Process.alive?(pid)
    end

    test "duplicate name returns error" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("dup-agent"))
      assert {:error, {:already_started, _}} = AgentSupervisor.spawn_agent(make_definition("dup-agent"))
    end

    test "stop_agent terminates the process" do
      {:ok, pid} = AgentSupervisor.spawn_agent(make_definition("stop-me"))
      assert :ok = AgentSupervisor.stop_agent("stop-me")
      refute Process.alive?(pid)
    end

    test "stop_agent returns error for unknown agent" do
      assert {:error, :not_found} = AgentSupervisor.stop_agent("nonexistent")
    end

    test "list_agents returns running agent names" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("list-a"))
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("list-b"))

      agents = AgentSupervisor.list_agents()
      assert "list-a" in agents
      assert "list-b" in agents
    end

    test "count_agents returns correct count" do
      assert AgentSupervisor.count_agents() == 0
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("count-1"))
      assert AgentSupervisor.count_agents() == 1
    end
  end

  describe "agent state" do
    test "get_state returns initial state" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("state-agent"))
      state = AgentServer.get_state("state-agent")

      assert state.definition.name == "state-agent"
      assert state.status == :idle
      assert state.health == :healthy
      assert state.history == []
      assert is_binary(state.session_id)
    end

    test "get_health returns :healthy initially" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("health-agent"))
      assert AgentServer.get_health("health-agent") == :healthy
    end
  end

  describe "task execution" do
    test "run_task returns placeholder result" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("task-agent"))

      assert {:ok, result} = AgentServer.run_task("task-agent", "do something")
      assert result.task == "do something"
      assert result.status == :pending_bridge
    end

    test "run_task records history" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("hist-agent"))

      AgentServer.run_task("hist-agent", "task 1")
      AgentServer.run_task("hist-agent", "task 2")

      state = AgentServer.get_state("hist-agent")
      assert length(state.history) == 2
      assert hd(state.history).event == :task_executed
    end
  end

  describe "messaging" do
    test "send_message records in history" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("msg-agent"))

      AgentServer.send_message("msg-agent", "hello")
      # Give cast time to process
      :timer.sleep(10)

      state = AgentServer.get_state("msg-agent")
      assert length(state.history) == 1
      assert hd(state.history).event == :message_received
    end
  end

  describe "health checks" do
    test "health check fires and updates last_health_check" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("hc-agent"))

      # Manually trigger health check
      [{pid, _}] = Registry.lookup(ZeroclawOrchestrator.AgentRegistry, "hc-agent")
      send(pid, :health_check)
      :timer.sleep(10)

      state = AgentServer.get_state("hc-agent")
      assert state.last_health_check != nil
    end
  end
end
