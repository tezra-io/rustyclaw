defmodule RustyclawOrchestrator.Tools.SpawnAgentToolTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentServer, AgentSupervisor}
  alias RustyclawOrchestrator.Tools.SpawnAgentTool

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  describe "execute/1" do
    test "spawns agent with minimal params" do
      assert {:ok, result} = SpawnAgentTool.execute(%{name: "spawn-basic"})
      assert result.agent_name == "spawn-basic"
      assert is_pid(result.pid)
      assert Process.alive?(result.pid)
    end

    test "spawns agent with all params" do
      params = %{
        name: "spawn-full",
        capabilities: ["search", "code"],
        persistent: true,
        model: "claude-sonnet-4-5",
        personality: "A test agent",
        delegates_to: ["helper"],
        max_memory_mb: 256
      }

      assert {:ok, _result} = SpawnAgentTool.execute(params)
      state = AgentServer.get_state("spawn-full")
      assert state.definition.capabilities == ["search", "code"]
      assert state.definition.persistent == true
      assert state.definition.model == "claude-sonnet-4-5"
      assert state.definition.max_memory_mb == 256
    end

    test "spawns agent with string keys" do
      assert {:ok, result} = SpawnAgentTool.execute(%{"name" => "spawn-string-keys"})
      assert result.agent_name == "spawn-string-keys"
    end

    test "spawns child with parent relationship" do
      {:ok, _} =
        AgentSupervisor.spawn_agent(%RustyclawOrchestrator.AgentDefinition{
          name: "tool-parent",
          capabilities: ["manage"],
          personality: "Parent"
        })

      assert {:ok, result} =
               SpawnAgentTool.execute(%{name: "tool-child", parent: "tool-parent"})

      child_state = AgentServer.get_state("tool-child")
      assert child_state.parent_pid != nil
      assert result.agent_name == "tool-child"
    end

    test "rejects missing name" do
      assert {:error, msg} = SpawnAgentTool.execute(%{capabilities: ["test"]})
      assert msg =~ "missing required"
    end

    test "rejects empty name" do
      assert {:error, msg} = SpawnAgentTool.execute(%{name: ""})
      assert msg =~ "empty"
    end

    test "rejects name with path separators" do
      assert {:error, msg} = SpawnAgentTool.execute(%{name: "bad/name"})
      assert msg =~ "path separators"
    end

    test "rejects non-map params" do
      assert {:error, "params must be a map"} = SpawnAgentTool.execute("not a map")
    end

    test "rejects duplicate agent name" do
      {:ok, _} = SpawnAgentTool.execute(%{name: "dup-tool-agent"})
      assert {:error, _} = SpawnAgentTool.execute(%{name: "dup-tool-agent"})
    end

    test "spawns with nonexistent parent gracefully (nil parent_pid)" do
      assert {:ok, _} = SpawnAgentTool.execute(%{name: "no-parent-agent", parent: "ghost"})
      state = AgentServer.get_state("no-parent-agent")
      assert state.parent_pid == nil
    end
  end
end
