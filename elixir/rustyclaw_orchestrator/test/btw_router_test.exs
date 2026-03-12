defmodule RustyclawOrchestrator.BtwRouterTest do
  @moduledoc """
  Tests for BtwRouter: prefix detection, stripping, and routing dispatch.
  """

  use ExUnit.Case

  alias RustyclawOrchestrator.{
    AgentDefinition,
    AgentServer,
    AgentSupervisor,
    BtwRouter
  }

  setup do
    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  # --- Prefix detection ---

  describe "btw_message?/1" do
    test "detects lowercase /btw prefix" do
      assert BtwRouter.btw_message?("/btw check the weather")
    end

    test "detects uppercase /BTW prefix" do
      assert BtwRouter.btw_message?("/BTW check the weather")
    end

    test "detects mixed case /Btw prefix" do
      assert BtwRouter.btw_message?("/Btw check the weather")
    end

    test "requires space after /btw" do
      refute BtwRouter.btw_message?("/btweet something")
    end

    test "rejects empty string" do
      refute BtwRouter.btw_message?("")
    end

    test "rejects short string" do
      refute BtwRouter.btw_message?("/btw")
    end

    test "rejects non-btw messages" do
      refute BtwRouter.btw_message?("hello world")
      refute BtwRouter.btw_message?("/help me")
      refute BtwRouter.btw_message?("btw check this")
    end

    test "handles /btw with single char after space" do
      assert BtwRouter.btw_message?("/btw x")
    end
  end

  # --- Prefix stripping ---

  describe "strip_prefix/1" do
    test "strips /btw prefix" do
      assert BtwRouter.strip_prefix("/btw check the weather") == "check the weather"
    end

    test "strips regardless of case (binary truncation)" do
      assert BtwRouter.strip_prefix("/BTW check this") == "check this"
    end

    test "returns original for non-btw messages" do
      assert BtwRouter.strip_prefix("hello world") == "hello world"
    end

    test "returns original for short strings" do
      assert BtwRouter.strip_prefix("/bt") == "/bt"
    end
  end

  # --- Routing ---

  describe "route/3" do
    test "routes non-btw message to main agent" do
      spawn_test_agent("route-main")
      assert {:main, :ok} = BtwRouter.route("route-main", "normal message")
    end

    test "routes non-btw message with provenance" do
      spawn_test_agent("route-prov")

      assert {:main, :ok} =
               BtwRouter.route("route-prov", "normal message", provenance: nil)
    end

    test "routes btw message to side-channel" do
      spawn_test_agent("route-btw")
      assert {:btw, pid} = BtwRouter.route("route-btw", "/btw check something")
      assert is_pid(pid)
    end

    test "btw routing passes channel_info" do
      spawn_test_agent("route-chan")

      channel_info = %{
        channel: "telegram",
        reply_to_message_id: 12_345,
        chat_id: "group-1"
      }

      assert {:btw, pid} =
               BtwRouter.route("route-chan", "/btw check it", channel_info: channel_info)

      assert is_pid(pid)
    end

    test "btw server terminates after execution" do
      spawn_test_agent("route-term")
      {:btw, pid} = BtwRouter.route("route-term", "/btw quick task")

      # Wait for the process to terminate
      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate within 5 seconds")
      end
    end

    test "btw routing snapshots agent context" do
      spawn_test_agent("route-ctx")
      AgentServer.update_accumulated_state("route-ctx", %{user_prefs: %{lang: "en"}})

      {:btw, pid} = BtwRouter.route("route-ctx", "/btw use my prefs")
      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate")
      end

      # Main agent state is unchanged
      state = AgentServer.get_state("route-ctx")
      assert state.accumulated_state == %{user_prefs: %{lang: "en"}}
    end

    test "main agent not interrupted during btw execution" do
      spawn_test_agent("route-noblock")

      # Start a btw task
      {:btw, _pid} = BtwRouter.route("route-noblock", "/btw background task")

      # Main agent should still be responsive immediately
      state = AgentServer.get_state("route-noblock")
      assert state.status == :idle
    end

    test "multiple concurrent btw tasks" do
      spawn_test_agent("route-multi")

      results =
        1..5
        |> Enum.map(fn i ->
          BtwRouter.route("route-multi", "/btw task #{i}")
        end)

      assert Enum.all?(results, fn {:btw, pid} -> is_pid(pid) end)

      pids = Enum.map(results, fn {:btw, pid} -> pid end)
      assert length(Enum.uniq(pids)) == 5
    end

    test "routes btw message when agent dies during context fetch" do
      spawn_test_agent("route-toctou")

      # Get the agent pid from the registry
      [{agent_pid, _}] =
        Registry.lookup(RustyclawOrchestrator.AgentRegistry, "route-toctou")

      # Kill the agent — Registry cleanup is async, so lookup may still
      # return the pid while get_state will exit. This exercises the
      # try/catch TOCTOU guard in fetch_agent_context/1.
      Process.exit(agent_pid, :kill)

      # Immediately route — should not crash regardless of timing
      result = BtwRouter.route("route-toctou", "/btw test toctou")

      case result do
        {:btw, pid} -> assert is_pid(pid)
        {:error, _} -> :ok
      end
    end

    test "route to nonexistent agent returns btw with empty context" do
      # BtwRouter fetches context gracefully even if agent doesn't exist
      # The BtwServer will fail at RustBridge call, not at routing
      {:btw, pid} = BtwRouter.route("nonexistent-agent", "/btw check")
      assert is_pid(pid)
    end
  end

  # --- Helpers ---

  defp spawn_test_agent(name) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: ["test"],
      personality: "BTW router test agent"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
    def_
  end
end
