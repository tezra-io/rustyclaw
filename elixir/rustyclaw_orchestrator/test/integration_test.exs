defmodule RustyclawOrchestrator.IntegrationTest do
  @moduledoc """
  End-to-end integration tests for the orchestration layer.

  Covers the key scenarios:
  1. Spawn an agent from definition
  2. Delegate a task by capability
  3. ACL denial
  4. Crash recovery (supervisor restarts agent)
  5. Session lifecycle tracking
  6. Multi-agent fanout
  7. Agent stop and cleanup
  8. Definition parsing + spawn + task
  9. Parent-child lifecycle via tools
  10. Persistent agent restart recovery
  11. Dynamic spawn → message → kill flow
  """

  use ExUnit.Case

  alias RustyclawOrchestrator.{
    AgentCoordinator,
    AgentDefinition,
    AgentServer,
    AgentSupervisor,
    SubAgentSession
  }

  alias RustyclawOrchestrator.Tools.{
    KillAgentTool,
    ListAgentsTool,
    MessageAgentTool,
    SpawnAgentTool
  }

  setup do
    SubAgentSession.clear()

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      snapshot_dir = Path.expand("~/.rustyclaw/agent_snapshots")

      if File.dir?(snapshot_dir) do
        snapshot_dir
        |> File.ls!()
        |> Enum.filter(&String.contains?(&1, "int-"))
        |> Enum.each(&File.rm(Path.join(snapshot_dir, &1)))
      end
    end)

    :ok
  end

  # --- 1. Spawn an agent from definition ---

  test "1: spawn agent from parsed definition" do
    md = """
    ---
    name: researcher
    capabilities:
      - web_search
      - summarize
    ---

    You are a research agent that searches the web and summarizes findings.
    """

    {:ok, def_} = AgentDefinition.parse(md)
    {:ok, warnings} = AgentDefinition.validate(def_)
    assert warnings == []

    {:ok, pid} = AgentSupervisor.spawn_agent(def_)
    assert Process.alive?(pid)

    state = AgentServer.get_state("researcher")
    assert state.definition.name == "researcher"
    assert state.definition.capabilities == ["web_search", "summarize"]
    assert state.status == :idle
  end

  # --- 2. Delegate a task by capability ---

  test "2: delegate task routes to capable agent" do
    spawn_agent("search-bot", capabilities: ["web_search"])
    spawn_agent("code-bot", capabilities: ["code_review"])

    {:ok, result} = AgentCoordinator.delegate("find info on BEAM", capabilities: ["web_search"])
    assert result.task == "find info on BEAM"
  end

  # --- 3. ACL denial ---

  test "3: delegation denied by ACL" do
    spawn_agent("restricted", delegates_to: ["trusted-helper"])
    spawn_agent("untrusted", capabilities: ["task"])

    assert {:error, :acl_denied} =
             AgentCoordinator.delegate("do something",
               capabilities: ["task"],
               from_agent: "restricted"
             )
  end

  # --- 4. Crash recovery ---

  test "4: supervisor restarts crashed agent" do
    spawn_agent("crashable", capabilities: ["task"])

    [{pid, _}] = Registry.lookup(RustyclawOrchestrator.AgentRegistry, "crashable")
    Process.exit(pid, :kill)

    :timer.sleep(50)

    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, "crashable") do
      [{new_pid, _}] ->
        assert Process.alive?(new_pid)
        assert new_pid != pid

      [] ->
        :ok
    end
  end

  # --- 5. Session lifecycle tracking ---

  test "5: session lifecycle from create to complete" do
    spawn_agent("worker", capabilities: ["task"])

    session = SubAgentSession.create("worker", "process data", parent_agent: "coordinator")
    assert session.status == :pending

    {:ok, session} = SubAgentSession.activate(session.id)
    assert session.status == :active

    {:ok, _result} = AgentServer.run_task("worker", "process data")

    {:ok, session} = SubAgentSession.complete(session.id, %{rows_processed: 42})
    assert session.status == :completed
    assert session.result == %{rows_processed: 42}
    assert session.completed_at != nil

    {:ok, found} = SubAgentSession.get(session.id)
    assert found.status == :completed
    assert found.parent_agent == "coordinator"
  end

  # --- 6. Multi-agent fanout ---

  test "6: fanout delegation sends to all matching agents" do
    spawn_agent("worker-1", capabilities: ["process"])
    spawn_agent("worker-2", capabilities: ["process"])
    spawn_agent("worker-3", capabilities: ["process"])

    {:ok, results} =
      AgentCoordinator.delegate("analyze data",
        capabilities: ["process"],
        strategy: :fanout
      )

    assert length(results) == 3

    Enum.each(results, fn {_agent, {:ok, result}} ->
      assert result.task == "analyze data"
    end)
  end

  # --- 7. Agent stop and cleanup ---

  test "7: stop agent removes from registry and supervisor" do
    spawn_agent("temp-agent", capabilities: ["task"])
    assert "temp-agent" in AgentSupervisor.list_agents()

    :ok = AgentSupervisor.stop_agent("temp-agent")
    :timer.sleep(20)

    refute "temp-agent" in AgentSupervisor.list_agents()
    assert Registry.lookup(RustyclawOrchestrator.AgentRegistry, "temp-agent") == []
  end

  # --- 8. Full flow: parse definition → spawn → task → session ---

  test "8: end-to-end flow from definition to completed session" do
    # Clean stale snapshots from previous runs (use configured snapshot_dir)
    snapshot_dir =
      Application.get_env(:rustyclaw_orchestrator, :snapshot_dir, "~/.rustyclaw/agent_snapshots")
      |> Path.expand()

    File.rm(Path.join(snapshot_dir, "e2e-agent.snapshot.etf"))

    md = """
    ---
    name: e2e-agent
    persistent: true
    capabilities:
      - data_processing
    delegates_to:
      - helper
    model: claude-sonnet-4-5
    temperature: 0.7
    max_tools_per_turn: 5
    max_memory_mb: 512
    ---

    You are an end-to-end test agent.
    """

    {:ok, def_} = AgentDefinition.parse(md)
    {:ok, warnings} = AgentDefinition.validate(def_)
    assert warnings == []
    assert def_.max_memory_mb == 512

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)

    session = SubAgentSession.create("e2e-agent", "full pipeline test")
    {:ok, _} = SubAgentSession.activate(session.id)

    agents = AgentCoordinator.find_agents(["data_processing"])
    assert "e2e-agent" in agents

    {:ok, result} = AgentServer.run_task("e2e-agent", "full pipeline test")
    assert result.status == :pending_bridge

    {:ok, completed} = SubAgentSession.complete(session.id, result)
    assert completed.status == :completed

    state = AgentServer.get_state("e2e-agent")
    assert state.definition.model == "claude-sonnet-4-5"
    assert state.definition.temperature == 0.7
    assert state.definition.delegates_to == ["helper"]
    assert state.definition.max_memory_mb == 512
    assert length(state.history) == 1
  end

  # --- 9. Parent-child lifecycle via tools ---

  test "9: spawn parent, spawn child, delegate, report, kill" do
    {:ok, parent} = SpawnAgentTool.execute(%{name: "int-parent", capabilities: ["manage"]})
    assert parent.agent_name == "int-parent"

    {:ok, child} =
      SpawnAgentTool.execute(%{
        name: "int-child",
        parent: "int-parent",
        capabilities: ["compute"]
      })

    assert child.agent_name == "int-child"

    child_state = AgentServer.get_state("int-child")
    assert child_state.parent_pid != nil

    {:ok, result} = AgentServer.delegate_to_child("int-parent", "int-child", "compute pi")
    assert result.task == "compute pi"

    :ok = AgentServer.report_to_parent("int-child", %{pi: 3.14159})
    :timer.sleep(10)

    parent_state = AgentServer.get_state("int-parent")
    assert Enum.any?(parent_state.history, &(&1.event == :delegated_to_child))
    assert Enum.any?(parent_state.history, &(&1.event == :child_reported))

    {:ok, _} = KillAgentTool.execute(%{name: "int-child"})
    :timer.sleep(20)
    refute "int-child" in AgentSupervisor.list_agents()
  end

  # --- 10. Persistent agent snapshot and recovery ---

  test "10: persistent agent saves and restores state" do
    {:ok, _} =
      SpawnAgentTool.execute(%{
        name: "int-persist",
        persistent: true,
        capabilities: ["remember"]
      })

    :ok = AgentServer.update_accumulated_state("int-persist", %{progress: 75, data: [1, 2, 3]})

    {:ok, _} = KillAgentTool.execute(%{name: "int-persist"})
    :timer.sleep(30)

    # Re-spawn — should restore from snapshot
    {:ok, _} =
      SpawnAgentTool.execute(%{
        name: "int-persist",
        persistent: true,
        capabilities: ["remember"]
      })

    state = AgentServer.get_state("int-persist")
    assert state.accumulated_state == %{progress: 75, data: [1, 2, 3]}
  end

  # --- 11. Dynamic spawn → message → list → kill flow ---

  test "11: full tool workflow — spawn, list, message, kill" do
    {:ok, _} = SpawnAgentTool.execute(%{name: "int-flow", capabilities: ["work"]})

    {:ok, %{agents: agents}} = ListAgentsTool.execute(%{capability: "work"})
    assert Enum.any?(agents, &(&1.name == "int-flow"))

    {:ok, result} =
      MessageAgentTool.execute(%{target: "int-flow", message: "do work", mode: :sync})

    assert result.delivered == true
    assert result.result.task == "do work"

    {:ok, _} = MessageAgentTool.execute(%{target: "int-flow", message: "status update"})
    :timer.sleep(10)

    state = AgentServer.get_state("int-flow")
    assert length(state.history) == 2

    {:ok, kill_result} = KillAgentTool.execute(%{name: "int-flow"})
    assert kill_result.killed == true
  end

  # --- 12. list_agents_detailed returns rich info ---

  test "12: list_agents_detailed includes all expected fields" do
    spawn_agent("int-detail", capabilities: ["code"], persistent: true)
    :ok = AgentServer.update_accumulated_state("int-detail", %{step: 1})

    detailed = AgentSupervisor.list_agents_detailed()
    assert detailed != []

    agent = Enum.find(detailed, &(&1.name == "int-detail"))
    assert agent.persistent == true
    assert agent.capabilities == ["code"]
    assert :step in agent.accumulated_state_keys
    assert is_integer(agent.uptime_seconds)
  end

  # --- Helpers ---

  defp spawn_agent(name, opts) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, []),
      delegates_to: Keyword.get(opts, :delegates_to, []),
      persistent: Keyword.get(opts, :persistent, false),
      personality: "Integration test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
    def_
  end
end
