defmodule RustyclawOrchestrator.Tools.ListAgentsToolTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentDefinition, AgentSupervisor}
  alias RustyclawOrchestrator.Tools.ListAgentsTool

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  defp spawn_agent(name, opts \\ []) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, []),
      persistent: Keyword.get(opts, :persistent, false),
      personality: "Test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
  end

  describe "execute/1" do
    test "returns empty list when no agents" do
      assert {:ok, %{agents: [], count: 0}} = ListAgentsTool.execute(%{})
    end

    test "lists all agents with basic info" do
      spawn_agent("list-1", capabilities: ["search"])
      spawn_agent("list-2", capabilities: ["code"])

      assert {:ok, %{agents: agents, count: 2}} = ListAgentsTool.execute(%{})
      names = Enum.map(agents, & &1.name)
      assert "list-1" in names
      assert "list-2" in names
    end

    test "returns detailed info when requested" do
      File.rm(Path.expand("~/.rustyclaw/agent_snapshots/detail-1.snapshot.etf"))
      spawn_agent("detail-1", capabilities: ["search"], persistent: true)

      assert {:ok, %{agents: [agent], count: 1}} =
               ListAgentsTool.execute(%{detailed: true})

      assert agent.name == "detail-1"
      assert agent.persistent == true
      assert is_list(agent.capabilities)
      assert is_integer(agent.uptime_seconds)
    end

    test "filters by capability" do
      spawn_agent("cap-a", capabilities: ["search"])
      spawn_agent("cap-b", capabilities: ["code"])
      spawn_agent("cap-c", capabilities: ["search", "code"])

      assert {:ok, %{agents: agents}} =
               ListAgentsTool.execute(%{capability: "search"})

      names = Enum.map(agents, & &1.name)
      assert "cap-a" in names
      assert "cap-c" in names
      refute "cap-b" in names
    end

    test "filters by status" do
      spawn_agent("status-1")
      spawn_agent("status-2")

      assert {:ok, %{agents: agents}} =
               ListAgentsTool.execute(%{status: :idle})

      assert length(agents) >= 2
    end

    test "accepts string keys" do
      spawn_agent("str-key")

      assert {:ok, %{agents: agents}} =
               ListAgentsTool.execute(%{"detailed" => false})

      assert length(agents) == 1
    end

    test "defaults to empty params" do
      spawn_agent("default-1")
      assert {:ok, %{count: 1}} = ListAgentsTool.execute()
    end
  end
end
