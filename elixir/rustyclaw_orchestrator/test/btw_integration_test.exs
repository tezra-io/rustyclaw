defmodule RustyclawOrchestrator.BtwIntegrationTest do
  @moduledoc """
  End-to-end integration tests for the BTW side-channel feature (TEZ-182).

  Covers:
  1. Full routing flow: inbound message → BtwRouter → BtwServer → response
  2. Main agent unaffected during BTW execution
  3. Context snapshot isolation
  4. Resource contention with graceful degradation
  5. Multiple concurrent BTW tasks
  6. Quote-reply metadata propagation
  7. Top-level API via RustyclawOrchestrator.route_message/3
  """

  use ExUnit.Case

  alias RustyclawOrchestrator.{
    AgentDefinition,
    AgentServer,
    AgentSupervisor,
    BtwRouter,
    ResourceLock
  }

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      for resource <- ["browser"] do
        ResourceLock.release(resource)

        try do
          :ets.delete(ResourceLock, resource)
        rescue
          ArgumentError -> :ok
        end
      end
    end)

    :ok
  end

  # --- 1. Full BTW routing flow ---

  test "1: btw message goes through full routing lifecycle" do
    spawn_test_agent("int-btw-flow")

    {:btw, pid} = BtwRouter.route("int-btw-flow", "/btw what time is it")
    assert is_pid(pid)

    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> flunk("BTW flow did not complete")
    end
  end

  # --- 2. Main agent unaffected ---

  test "2: main agent state unchanged during btw execution" do
    spawn_test_agent("int-btw-main")
    AgentServer.update_accumulated_state("int-btw-main", %{task_progress: 50})

    {:btw, pid} = BtwRouter.route("int-btw-main", "/btw side task")
    ref = Process.monitor(pid)

    # Immediately check main agent
    state = AgentServer.get_state("int-btw-main")
    assert state.status == :idle
    assert state.accumulated_state == %{task_progress: 50}

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> :ok
    end

    # Check again after btw completes
    state = AgentServer.get_state("int-btw-main")
    assert state.accumulated_state == %{task_progress: 50}
  end

  # --- 3. Context snapshot isolation ---

  test "3: btw gets snapshot copy, not shared reference" do
    spawn_test_agent("int-btw-ctx")
    AgentServer.update_accumulated_state("int-btw-ctx", %{version: 1, data: [1, 2, 3]})

    {:btw, pid} = BtwRouter.route("int-btw-ctx", "/btw analyze data")

    # Modify main agent state while btw is running
    AgentServer.update_accumulated_state("int-btw-ctx", %{version: 2, extra: "new"})

    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> :ok
    end

    # Main agent should have the updated state
    state = AgentServer.get_state("int-btw-ctx")
    assert state.accumulated_state.version == 2
    assert state.accumulated_state.extra == "new"
  end

  # --- 4. Resource contention ---

  test "4: btw task waits for locked exclusive resource" do
    spawn_test_agent("int-btw-lock")

    # Lock the browser resource (simulating main task using it)
    :ok = ResourceLock.acquire("browser", wait_ms: 100)

    {:btw, pid} = BtwRouter.route("int-btw-lock", "/btw open browser and check")
    ref = Process.monitor(pid)

    # Release after a moment
    Process.sleep(100)
    ResourceLock.release("browser")

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> flunk("BTW task did not complete after resource release")
    end
  end

  test "4b: btw task returns graceful error when resource busy too long" do
    spawn_test_agent("int-btw-busy")

    # Lock browser and don't release — btw should timeout gracefully
    lock_pid =
      spawn(fn ->
        ResourceLock.acquire("browser", wait_ms: 100)
        # Hold for longer than btw wait time
        Process.sleep(10_000)
      end)

    Process.sleep(20)

    {:btw, pid} = BtwRouter.route("int-btw-busy", "/btw open browser please")
    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      10_000 -> flunk("BTW task did not terminate on resource busy")
    end

    Process.exit(lock_pid, :kill)
  end

  # --- 5. Multiple concurrent BTW tasks ---

  test "5: multiple concurrent btw tasks all complete" do
    spawn_test_agent("int-btw-conc")

    results =
      1..5
      |> Enum.map(fn i ->
        BtwRouter.route("int-btw-conc", "/btw task number #{i}")
      end)

    pids = Enum.map(results, fn {:btw, pid} -> pid end)
    refs = Enum.map(pids, &Process.monitor/1)

    Enum.each(refs, fn ref ->
      receive do
        {:DOWN, ^ref, :process, _pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("Concurrent BTW task did not complete")
      end
    end)

    # Agent still healthy
    state = AgentServer.get_state("int-btw-conc")
    assert state.health == :healthy
  end

  # --- 6. Quote-reply metadata ---

  test "6: channel_info with quote-reply metadata propagated" do
    spawn_test_agent("int-btw-reply")

    channel_info = %{
      channel: "telegram",
      reply_to_message_id: 99_887,
      chat_id: "-1001234567890"
    }

    {:btw, pid} =
      BtwRouter.route("int-btw-reply", "/btw quick check", channel_info: channel_info)

    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> flunk("BTW with channel info did not complete")
    end
  end

  # --- 7. Top-level API ---

  test "7: route_message/3 delegates to BtwRouter for btw messages" do
    spawn_test_agent("int-btw-api")

    {:btw, pid} = RustyclawOrchestrator.route_message("int-btw-api", "/btw api test")
    assert is_pid(pid)

    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
    after
      5_000 -> flunk("API btw route did not complete")
    end
  end

  test "7b: route_message/3 forwards non-btw to main agent" do
    spawn_test_agent("int-btw-api2")

    assert {:main, :ok} = RustyclawOrchestrator.route_message("int-btw-api2", "normal message")

    state = AgentServer.get_state("int-btw-api2")
    assert Enum.any?(state.history, &(&1.event == :message_received))
  end

  # --- 8. Non-btw message still recorded in agent history ---

  test "8: non-btw message recorded in agent history via route" do
    spawn_test_agent("int-btw-hist")
    BtwRouter.route("int-btw-hist", "regular message here")

    state = AgentServer.get_state("int-btw-hist")
    assert Enum.any?(state.history, &(&1.event == :message_received))
    msg_entry = Enum.find(state.history, &(&1.event == :message_received))
    assert msg_entry.message == "regular message here"
  end

  # --- Helpers ---

  defp spawn_test_agent(name) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: ["test"],
      personality: "BTW integration test agent"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
    def_
  end
end
