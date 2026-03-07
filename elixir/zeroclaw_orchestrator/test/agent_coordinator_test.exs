defmodule ZeroclawOrchestrator.AgentCoordinatorTest do
  use ExUnit.Case

  alias ZeroclawOrchestrator.{AgentCoordinator, AgentDefinition, AgentSupervisor}

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  defp spawn_agent(name, opts) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, []),
      delegates_to: Keyword.get(opts, :delegates_to, []),
      personality: "Test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
    def_
  end

  describe "find_agents/1" do
    test "finds agents with matching capabilities" do
      spawn_agent("web-agent", capabilities: ["web_search", "summarize"])
      spawn_agent("code-agent", capabilities: ["code_review"])

      assert ["web-agent"] = AgentCoordinator.find_agents(["web_search"])
      assert ["code-agent"] = AgentCoordinator.find_agents(["code_review"])
    end

    test "requires all capabilities to match" do
      spawn_agent("partial", capabilities: ["web_search"])
      spawn_agent("full", capabilities: ["web_search", "summarize"])

      matches = AgentCoordinator.find_agents(["web_search", "summarize"])
      assert matches == ["full"]
    end

    test "empty capabilities matches all agents" do
      spawn_agent("a", capabilities: ["x"])
      spawn_agent("b", capabilities: ["y"])

      matches = AgentCoordinator.find_agents([])
      assert length(matches) == 2
    end

    test "returns empty list when no agents match" do
      spawn_agent("a", capabilities: ["x"])
      assert AgentCoordinator.find_agents(["nonexistent"]) == []
    end
  end

  describe "delegation ACL" do
    test "empty delegates_to allows all" do
      spawn_agent("source", delegates_to: [])
      assert AgentCoordinator.allowed_to_delegate?("source", "anyone") == true
    end

    test "explicit delegates_to restricts targets" do
      spawn_agent("source", delegates_to: ["helper"])
      assert AgentCoordinator.allowed_to_delegate?("source", "helper") == true
      assert AgentCoordinator.allowed_to_delegate?("source", "other") == false
    end

    test "nonexistent source agent returns false" do
      assert AgentCoordinator.allowed_to_delegate?("ghost", "target") == false
    end
  end

  describe "delegate/2" do
    test "first_available routes to first matching agent" do
      spawn_agent("worker", capabilities: ["task"])

      assert {:ok, result} = AgentCoordinator.delegate("do it", capabilities: ["task"])
      assert result.task == "do it"
    end

    test "returns no_matching_agents when none match" do
      assert {:error, :no_matching_agents} =
               AgentCoordinator.delegate("do it", capabilities: ["nonexistent"])
    end

    test "returns acl_denied when ACL blocks delegation" do
      spawn_agent("source", delegates_to: ["helper"])
      spawn_agent("blocked-target", capabilities: ["task"])

      assert {:error, :acl_denied} =
               AgentCoordinator.delegate("do it",
                 capabilities: ["task"],
                 from_agent: "source"
               )
    end

    test "ACL allows delegation to permitted agent" do
      spawn_agent("source", delegates_to: ["worker"])
      spawn_agent("worker", capabilities: ["task"])

      assert {:ok, _} =
               AgentCoordinator.delegate("do it",
                 capabilities: ["task"],
                 from_agent: "source"
               )
    end

    test "sequential strategy tries agents in order" do
      spawn_agent("seq-1", capabilities: ["task"])
      spawn_agent("seq-2", capabilities: ["task"])

      assert {:ok, _} =
               AgentCoordinator.delegate("do it",
                 capabilities: ["task"],
                 strategy: :sequential
               )
    end

    test "fanout strategy returns results from all agents" do
      spawn_agent("fan-1", capabilities: ["task"])
      spawn_agent("fan-2", capabilities: ["task"])

      assert {:ok, results} =
               AgentCoordinator.delegate("do it",
                 capabilities: ["task"],
                 strategy: :fanout
               )

      assert length(results) == 2
    end
  end
end
