defmodule RustyclawOrchestrator.Tools.MessageAgentToolTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{AgentDefinition, AgentServer, AgentSupervisor}
  alias RustyclawOrchestrator.TestSupport.BridgeMock
  alias RustyclawOrchestrator.Tools.MessageAgentTool

  setup do
    BridgeMock.setup()

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  defp spawn_agent(name) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: ["test"],
      personality: "Test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
  end

  describe "execute/1 async mode" do
    test "delivers async message to agent" do
      spawn_agent("msg-target")

      assert {:ok, result} =
               MessageAgentTool.execute(%{target: "msg-target", message: "hello"})

      assert result.delivered == true
      assert result.mode == :async

      :timer.sleep(10)
      state = AgentServer.get_state("msg-target")
      assert Enum.any?(state.history, &(&1.event == :message_received))
    end

    test "returns error for nonexistent target" do
      assert {:error, msg} =
               MessageAgentTool.execute(%{target: "ghost", message: "hello"})

      assert msg =~ "not found"
    end
  end

  describe "execute/1 sync mode" do
    test "delivers sync task and returns result" do
      spawn_agent("sync-target")

      assert {:ok, result} =
               MessageAgentTool.execute(%{
                 target: "sync-target",
                 message: "do work",
                 mode: :sync
               })

      assert result.delivered == true
      assert result.mode == :sync
      assert result.result["task"] == "do work"
    end

    test "sync mode with string mode key" do
      spawn_agent("sync-str")

      assert {:ok, result} =
               MessageAgentTool.execute(%{
                 "target" => "sync-str",
                 "message" => "work",
                 "mode" => "sync"
               })

      assert result.mode == :sync
    end
  end

  describe "validation" do
    test "rejects missing target" do
      assert {:error, msg} = MessageAgentTool.execute(%{message: "hello"})
      assert msg =~ "target"
    end

    test "rejects missing message" do
      assert {:error, msg} = MessageAgentTool.execute(%{target: "someone"})
      assert msg =~ "message"
    end

    test "rejects empty target" do
      assert {:error, msg} = MessageAgentTool.execute(%{target: "", message: "hello"})
      assert msg =~ "empty"
    end

    test "rejects empty message" do
      assert {:error, msg} = MessageAgentTool.execute(%{target: "someone", message: ""})
      assert msg =~ "empty"
    end

    test "rejects non-map params" do
      assert {:error, "params must be a map"} = MessageAgentTool.execute("not a map")
    end

    test "accepts string keys" do
      spawn_agent("str-msg-target")

      assert {:ok, _} =
               MessageAgentTool.execute(%{"target" => "str-msg-target", "message" => "hi"})
    end
  end
end
