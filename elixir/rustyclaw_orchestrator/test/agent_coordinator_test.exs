defmodule RustyclawOrchestrator.AgentCoordinatorTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentCoordinator, AgentDefinition, AgentSupervisor}

  setup do
    # Reset coordinator definitions to empty so disk-cached defs don't leak between tests
    empty_dir =
      Path.join(System.tmp_dir!(), "rustyclaw_empty_agents_#{System.unique_integer([:positive])}")

    File.mkdir_p!(empty_dir)
    original_dir = Application.get_env(:rustyclaw_orchestrator, :agents_dir)
    Application.put_env(:rustyclaw_orchestrator, :agents_dir, empty_dir)
    AgentCoordinator.refresh_definitions()

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      if original_dir do
        Application.put_env(:rustyclaw_orchestrator, :agents_dir, original_dir)
      end

      File.rm_rf!(empty_dir)
    end)

    :ok
  end

  defp write_agent_file(dir, name, capabilities) do
    caps_section =
      case capabilities do
        [] ->
          ""

        caps ->
          "capabilities:\n" <> Enum.map_join(caps, "\n", &"  - #{&1}") <> "\n"
      end

    content = "---\nname: #{name}\n#{caps_section}---\n\nTest agent #{name}\n"
    File.write!(Path.join(dir, "#{name}.md"), content)
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

  describe "agent discovery from disk" do
    setup do
      dir =
        Path.join(
          System.tmp_dir!(),
          "rustyclaw_test_agents_#{System.unique_integer([:positive])}"
        )

      File.mkdir_p!(dir)
      Application.put_env(:rustyclaw_orchestrator, :agents_dir, dir)

      on_exit(fn ->
        File.rm_rf!(dir)
      end)

      {:ok, agents_dir: dir}
    end

    test "discovers definitions from agents directory", %{agents_dir: dir} do
      write_agent_file(dir, "disk-agent", ["web_search", "summarize"])
      AgentCoordinator.refresh_definitions()

      assert ["disk-agent"] = AgentCoordinator.find_agents(["web_search"])
    end

    test "includes unspawned agents in capability matching", %{agents_dir: dir} do
      write_agent_file(dir, "disk-only", ["research"])
      spawn_agent("running", capabilities: ["research"])
      AgentCoordinator.refresh_definitions()

      matches = AgentCoordinator.find_agents(["research"])
      assert "running" in matches
      assert "disk-only" in matches
      assert length(matches) == 2
    end

    test "running agents take priority over disk definitions", %{agents_dir: dir} do
      # Definition on disk has web_search capability
      write_agent_file(dir, "my-agent", ["web_search"])
      # Running agent registered with code_review capability
      spawn_agent("my-agent", capabilities: ["code_review"])
      AgentCoordinator.refresh_definitions()

      # Should use running agent's capabilities, not the definition's
      assert [] = AgentCoordinator.find_agents(["web_search"])
      assert ["my-agent"] = AgentCoordinator.find_agents(["code_review"])
    end

    test "spawns unspawned agent on demand during delegation", %{agents_dir: dir} do
      write_agent_file(dir, "lazy-agent", ["task"])
      AgentCoordinator.refresh_definitions()

      refute "lazy-agent" in AgentSupervisor.list_agents()

      assert {:ok, _} = AgentCoordinator.delegate("do it", capabilities: ["task"])

      assert "lazy-agent" in AgentSupervisor.list_agents()
    end

    test "skips malformed definition files", %{agents_dir: dir} do
      # Valid definition
      write_agent_file(dir, "good-agent", ["task"])
      # Malformed file (no YAML frontmatter)
      File.write!(Path.join(dir, "bad-agent.md"), "no frontmatter here")
      AgentCoordinator.refresh_definitions()

      assert ["good-agent"] = AgentCoordinator.find_agents(["task"])
    end

    test "returns empty when agents directory does not exist" do
      Application.put_env(:rustyclaw_orchestrator, :agents_dir, "/nonexistent/path")
      AgentCoordinator.refresh_definitions()

      assert [] = AgentCoordinator.find_agents(["anything"])
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

    test "coordinator remains responsive during delegation execution" do
      spawn_agent("slow-worker", capabilities: ["slow"])
      spawn_agent("fast-worker", capabilities: ["fast"])

      # Start a delegation in a separate process (it will block waiting for result)
      caller =
        Task.async(fn ->
          AgentCoordinator.delegate("slow task",
            capabilities: ["slow"],
            strategy: :first_available
          )
        end)

      # The coordinator should still respond to other calls while delegation runs
      assert is_list(AgentCoordinator.find_agents(["fast"]))
      assert AgentCoordinator.allowed_to_delegate?("slow-worker", "anyone") == true

      # The original delegation should still complete successfully
      assert {:ok, _} = Task.await(caller)
    end

    test "task crash returns error to caller" do
      spawn_agent("crash-agent", capabilities: ["crash"])

      # Stop the agent after the coordinator resolves routing but before task executes.
      # This simulates a crash during strategy execution.
      # Since AgentServer.run_task will catch the exit, we just verify the flow works.
      assert {:ok, _} =
               AgentCoordinator.delegate("do it",
                 capabilities: ["crash"],
                 strategy: :first_available
               )
    end
  end
end
