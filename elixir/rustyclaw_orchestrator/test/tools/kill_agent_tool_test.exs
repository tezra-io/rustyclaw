defmodule RustyclawOrchestrator.Tools.KillAgentToolTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer, AgentSupervisor}
  alias RustyclawOrchestrator.Tools.KillAgentTool

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      snapshot_dir = Path.expand("~/.rustyclaw/agent_snapshots")

      if File.dir?(snapshot_dir) do
        snapshot_dir
        |> File.ls!()
        |> Enum.filter(&String.contains?(&1, "kill"))
        |> Enum.each(&File.rm(Path.join(snapshot_dir, &1)))
      end
    end)

    :ok
  end

  defp spawn_agent(name, opts \\ []) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, ["test"]),
      persistent: Keyword.get(opts, :persistent, false),
      personality: "Test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
  end

  describe "execute/1" do
    test "kills a running agent" do
      spawn_agent("kill-me")
      assert "kill-me" in AgentSupervisor.list_agents()

      assert {:ok, result} = KillAgentTool.execute(%{name: "kill-me"})
      assert result.killed == true
      assert result.agent_name == "kill-me"

      :timer.sleep(20)
      refute "kill-me" in AgentSupervisor.list_agents()
    end

    test "saves snapshot for persistent agent before kill" do
      spawn_agent("kill-persist", persistent: true)
      :ok = AgentServer.update_accumulated_state("kill-persist", %{important: "data"})

      assert {:ok, result} = KillAgentTool.execute(%{name: "kill-persist"})
      assert result.snapshot_saved == true
    end

    test "does not save snapshot for non-persistent agent" do
      spawn_agent("kill-ephemeral")

      assert {:ok, result} = KillAgentTool.execute(%{name: "kill-ephemeral"})
      assert result.snapshot_saved == false
    end

    test "returns error for nonexistent agent" do
      assert {:error, msg} = KillAgentTool.execute(%{name: "ghost"})
      assert msg =~ "not found"
    end

    test "rejects missing name" do
      assert {:error, msg} = KillAgentTool.execute(%{})
      assert msg =~ "missing required"
    end

    test "rejects empty name" do
      assert {:error, msg} = KillAgentTool.execute(%{name: ""})
      assert msg =~ "empty"
    end

    test "rejects non-map params" do
      assert {:error, "params must be a map"} = KillAgentTool.execute("not a map")
    end

    test "accepts string keys" do
      spawn_agent("kill-str")
      assert {:ok, result} = KillAgentTool.execute(%{"name" => "kill-str"})
      assert result.killed == true
    end
  end
end
