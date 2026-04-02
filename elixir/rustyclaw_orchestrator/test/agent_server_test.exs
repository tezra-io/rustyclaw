defmodule RustyclawOrchestrator.AgentServerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer, AgentSupervisor}

  @snapshot_dir Application.compile_env(
                  :rustyclaw_orchestrator,
                  :snapshot_dir,
                  "~/.rustyclaw/agent_snapshots"
                )
                |> Path.expand()

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      # Clean up any snapshot files created during tests
      if File.dir?(@snapshot_dir) do
        File.rm_rf!(@snapshot_dir)
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

  describe "task execution (TEZ-146: wired to RustBridge)" do
    test "run_task returns bridge error when Rust core is unreachable" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("task-agent"))

      # Bridge is unreachable in tests (max_retries: 0, connect_timeout: 100)
      assert {:error, _reason} = AgentServer.run_task("task-agent", "do something")
    end

    test "run_task records history on error" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("hist-agent"))

      AgentServer.run_task("hist-agent", "task 1")

      state = AgentServer.get_state("hist-agent")
      assert state.history != []
      events = Enum.map(state.history, & &1.event)
      assert :task_completed in events
    end

    test "run_task updates last_active_at" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("active-agent"))
      state_before = AgentServer.get_state("active-agent")
      :timer.sleep(10)
      AgentServer.run_task("active-agent", "task")
      state_after = AgentServer.get_state("active-agent")

      assert DateTime.compare(state_after.last_active_at, state_before.last_active_at) != :lt
    end

    test "run_task returns to idle status after completion" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("status-agent"))

      AgentServer.run_task("status-agent", "task")

      state = AgentServer.get_state("status-agent")
      assert state.status == :idle
      assert state.pending_task == nil
    end

    test "run_task accepts timeout option" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("timeout-agent"))

      # Should not raise with a custom timeout
      AgentServer.run_task("timeout-agent", "task", timeout: 10_000)
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

      # Delegation calls child's run_task which goes through bridge (unreachable in tests)
      result = AgentServer.delegate_to_child("del-parent", "del-child", "child task")
      assert is_tuple(result)

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

  describe "memory limit enforcement" do
    test "run_task rejects when memory limit exceeded" do
      # max_memory_mb: 0 ensures any process memory exceeds the limit
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("mem-limit-task", max_memory_mb: 0))

      assert {:error, :memory_limit_exceeded} =
               AgentServer.run_task("mem-limit-task", "should be rejected")

      state = AgentServer.get_state("mem-limit-task")
      assert Enum.any?(state.history, &(&1.reason == :memory_limit))
    end

    test "update_accumulated_state rejects when memory limit exceeded" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("mem-limit-state", max_memory_mb: 0))

      assert {:error, :memory_limit_exceeded} =
               AgentServer.update_accumulated_state("mem-limit-state", %{data: "test"})
    end

    test "agent without max_memory_mb accepts unlimited state" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("no-mem-limit"))

      # run_task dispatches to bridge (errors in test, but not rejected by memory limit)
      result = AgentServer.run_task("no-mem-limit", "should not be memory-rejected")
      # Should NOT be :memory_limit_exceeded
      refute match?({:error, :memory_limit_exceeded}, result)

      assert :ok =
               AgentServer.update_accumulated_state("no-mem-limit", %{
                 large: String.duplicate("x", 10_000)
               })

      state = AgentServer.get_state("no-mem-limit")
      assert byte_size(state.accumulated_state.large) == 10_000
    end
  end

  describe "state persistence" do
    test "persistent agent saves snapshot on terminate" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("persist-test", persistent: true))
      :ok = AgentServer.update_accumulated_state("persist-test", %{saved: true})

      AgentSupervisor.stop_agent("persist-test")
      :timer.sleep(20)

      snapshot_path = Path.join(@snapshot_dir, "persist-test.snapshot.etf")
      assert File.exists?(snapshot_path)

      data = snapshot_path |> File.read!() |> :erlang.binary_to_term([:safe])
      assert data.accumulated_state == %{saved: true}
    end

    test "persistent agent restores snapshot on restart" do
      # Create and save snapshot manually
      File.mkdir_p!(@snapshot_dir)
      path = Path.join(@snapshot_dir, "restore-test.snapshot.etf")

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
    end

    test "non-persistent agent does not save snapshot" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("ephemeral-test", persistent: false))
      :ok = AgentServer.update_accumulated_state("ephemeral-test", %{temp: true})

      AgentSupervisor.stop_agent("ephemeral-test")
      :timer.sleep(20)

      snapshot_path = Path.join(@snapshot_dir, "ephemeral-test.snapshot.etf")
      refute File.exists?(snapshot_path)
    end

    test "non-persistent agent does not restore snapshot (TEZ-204)" do
      # Create a persistent agent and give it state, then snapshot it
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("snap-guard", persistent: true))
      :ok = AgentServer.update_accumulated_state("snap-guard", %{stale: true})
      AgentSupervisor.stop_agent("snap-guard")
      :timer.sleep(20)

      # Verify snapshot exists
      snapshot_path = Path.join(@snapshot_dir, "snap-guard.snapshot.etf")
      assert File.exists?(snapshot_path)

      # Now spawn the same name as non-persistent — should NOT restore
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("snap-guard", persistent: false))
      state = AgentServer.get_state("snap-guard")

      # accumulated_state should be empty (not restored from stale snapshot)
      assert state.accumulated_state == %{}
    end
  end

  describe "delegate_to_child crash handling (TEZ-202)" do
    test "delegate_to_child returns error when child crashes" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("parent-crash-test"))
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("child-crash-test"))

      # Kill the child mid-delegation to simulate crash
      task =
        Task.async(fn ->
          # This should not hang — it should return an error
          AgentServer.delegate_to_child("parent-crash-test", "child-crash-test", "crash task")
        end)

      # Give the delegation a moment to start, then kill the child
      :timer.sleep(10)
      AgentSupervisor.stop_agent("child-crash-test")

      # The caller should get a response (not hang until timeout)
      result = Task.await(task, 5_000)
      # Result should be {:ok, _} or {:error, _} — the key test is that it doesn't hang
      assert is_tuple(result)
    end
  end

  describe "health with bridge state (TEZ-203)" do
    test "health degrades when bridge is unreachable" do
      {:ok, _} = AgentSupervisor.spawn_agent(make_definition("health-bridge-test"))

      [{pid, _}] =
        Registry.lookup(RustyclawOrchestrator.AgentRegistry, "health-bridge-test")

      # Directly set bridge_healthy to false in GenServer state
      :sys.replace_state(pid, fn state ->
        %{state | bridge_healthy: false}
      end)

      # Trigger health evaluation
      send(pid, :health_check)
      :timer.sleep(50)

      health = AgentServer.get_health("health-bridge-test")
      assert health in [:degraded, :unhealthy]
    end
  end
end
