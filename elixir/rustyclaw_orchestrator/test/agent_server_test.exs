defmodule RustyclawOrchestrator.AgentServerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer, AgentSupervisor}

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      # Clean up any snapshot files created during tests
      snapshot_dir = Path.expand("~/.rustyclaw/agent_snapshots")

      if File.dir?(snapshot_dir) do
        snapshot_dir
        |> File.ls!()
        |> Enum.filter(&String.contains?(&1, "test"))
        |> Enum.each(&File.rm(Path.join(snapshot_dir, &1)))
      end
    end)

    :ok
  end

  defp make_definition(name, opts \\ []) do
    %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, ["test"]),
      persistent: Keyword.get(opts, :persistent, false),
      parent: Keyword.get(opts, :parent),
      max_memory_mb: Keyword.get(opts, :max_memory_mb),
      delegates_to: Keyword.get(opts, :delegates_to, []),
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

      assert [{pid, _}] = Registry.lookup(RustyclawOrchestrator.AgentRegistry, "named-agent")
      assert Process.alive?(pid)
    end

    test "duplicate name returns error" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("dup-agent"))

      assert {:error, {:already_started, _}} =
               AgentSupervisor.spawn_agent(make_definition("dup-agent"))
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
    test "get_state returns initial state with new fields" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("state-agent"))
      state = AgentServer.get_state("state-agent")

      assert state.definition.name == "state-agent"
      assert state.status == :idle
      assert state.health == :healthy
      assert state.history == []
      assert state.accumulated_state == %{}
      assert state.parent_pid == nil
      assert state.child_pids == []
      assert is_binary(state.session_id)
      assert %DateTime{} = state.last_active_at
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

    test "run_task updates last_active_at" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("active-agent"))
      state_before = AgentServer.get_state("active-agent")
      :timer.sleep(10)
      AgentServer.run_task("active-agent", "task")
      state_after = AgentServer.get_state("active-agent")

      assert DateTime.compare(state_after.last_active_at, state_before.last_active_at) != :lt
    end
  end

  describe "messaging" do
    test "send_message records in history" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("msg-agent"))

      AgentServer.send_message("msg-agent", "hello")
      :timer.sleep(10)

      state = AgentServer.get_state("msg-agent")
      assert length(state.history) == 1
      assert hd(state.history).event == :message_received
    end
  end

  describe "health checks" do
    test "health check fires and updates last_health_check" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("hc-agent"))

      [{pid, _}] = Registry.lookup(RustyclawOrchestrator.AgentRegistry, "hc-agent")
      send(pid, :health_check)
      :timer.sleep(10)

      state = AgentServer.get_state("hc-agent")
      assert state.last_health_check != nil
    end
  end

  describe "accumulated state" do
    test "update_accumulated_state merges new data" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("acc-agent"))

      :ok = AgentServer.update_accumulated_state("acc-agent", %{step: 1, data: "initial"})
      state = AgentServer.get_state("acc-agent")
      assert state.accumulated_state == %{step: 1, data: "initial"}

      :ok = AgentServer.update_accumulated_state("acc-agent", %{step: 2, extra: "new"})
      state = AgentServer.get_state("acc-agent")
      assert state.accumulated_state == %{step: 2, data: "initial", extra: "new"}
    end

    test "get_snapshot returns accumulated state" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("snap-agent"))
      :ok = AgentServer.update_accumulated_state("snap-agent", %{progress: 50})

      snapshot = AgentServer.get_snapshot("snap-agent")
      assert snapshot.agent_name == "snap-agent"
      assert snapshot.accumulated_state == %{progress: 50}
      assert %DateTime{} = snapshot.snapshot_at
    end
  end

  describe "parent-child relationships" do
    test "spawn with parent_pid establishes relationship" do
      {:ok, parent_pid} = AgentSupervisor.spawn_agent(make_definition("parent-agent"))

      {:ok, _child_pid} =
        AgentSupervisor.spawn_agent(make_definition("child-agent"), parent_pid: parent_pid)

      child_state = AgentServer.get_state("child-agent")
      assert child_state.parent_pid == parent_pid
    end

    test "delegate_to_child sends task and records history" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("del-parent"))
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("del-child"))

      {:ok, result} = AgentServer.delegate_to_child("del-parent", "del-child", "child task")
      assert result.task == "child task"

      parent_state = AgentServer.get_state("del-parent")
      assert Enum.any?(parent_state.history, &(&1.event == :delegated_to_child))
    end

    test "delegate_to_child returns error for missing child" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("lonely-parent"))

      assert {:error, :child_not_found} =
               AgentServer.delegate_to_child("lonely-parent", "nonexistent", "task")
    end

    test "report_to_parent sends result to parent" do
      {:ok, parent_pid} = AgentSupervisor.spawn_agent(make_definition("rpt-parent"))
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("rpt-child"), parent_pid: parent_pid)

      assert :ok = AgentServer.report_to_parent("rpt-child", %{answer: 42})
      :timer.sleep(10)

      parent_state = AgentServer.get_state("rpt-parent")
      assert Enum.any?(parent_state.history, &(&1.event == :child_reported))
    end

    test "report_to_parent returns error when no parent" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("orphan"))
      assert {:error, :no_parent} = AgentServer.report_to_parent("orphan", %{result: "data"})
    end

    test "child monitors parent — parent_pid cleared on parent exit" do
      {:ok, parent_pid} = AgentSupervisor.spawn_agent(make_definition("mon-parent"))
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("mon-child"), parent_pid: parent_pid)

      AgentSupervisor.stop_agent("mon-parent")
      :timer.sleep(30)

      child_state = AgentServer.get_state("mon-child")
      assert child_state.parent_pid == nil
    end
  end

  describe "state persistence" do
    test "persistent agent saves snapshot on terminate" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("persist-test", persistent: true))
      :ok = AgentServer.update_accumulated_state("persist-test", %{saved: true})

      AgentSupervisor.stop_agent("persist-test")
      :timer.sleep(20)

      snapshot_path = Path.expand("~/.rustyclaw/agent_snapshots/persist-test.snapshot.etf")
      assert File.exists?(snapshot_path)

      data = snapshot_path |> File.read!() |> :erlang.binary_to_term([:safe])
      assert data.accumulated_state == %{saved: true}
    end

    test "persistent agent restores snapshot on restart" do
      # Create and save snapshot manually
      snapshot_dir = Path.expand("~/.rustyclaw/agent_snapshots")
      File.mkdir_p!(snapshot_dir)
      path = Path.join(snapshot_dir, "restore-test.snapshot.etf")

      snapshot = %{
        accumulated_state: %{restored: true, step: 5},
        history: [%{event: :old_event, timestamp: DateTime.utc_now()}]
      }

      File.write!(path, :erlang.term_to_binary(snapshot))

      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("restore-test", persistent: true))
      state = AgentServer.get_state("restore-test")

      assert state.accumulated_state == %{restored: true, step: 5}
      assert length(state.history) == 1
      assert hd(state.history).event == :old_event

      File.rm(path)
    end

    test "non-persistent agent does not save snapshot" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("ephemeral-test", persistent: false))
      :ok = AgentServer.update_accumulated_state("ephemeral-test", %{temp: true})

      AgentSupervisor.stop_agent("ephemeral-test")
      :timer.sleep(20)

      snapshot_path = Path.expand("~/.rustyclaw/agent_snapshots/ephemeral-test.snapshot.etf")
      refute File.exists?(snapshot_path)
    end
  end
end
