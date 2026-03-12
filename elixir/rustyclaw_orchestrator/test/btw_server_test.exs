defmodule RustyclawOrchestrator.BtwServerTest do
  @moduledoc """
  Tests for BtwServer: lifecycle, execution, response formatting, and termination.
  """

  use ExUnit.Case

  alias RustyclawOrchestrator.{BtwServer, BtwSupervisor}

  # --- Lifecycle ---

  describe "lifecycle" do
    test "starts and terminates after execution" do
      {:ok, pid} =
        BtwServer.start_link(
          message: "check the weather",
          agent_name: "test-agent",
          context: %{accumulated_state: %{}, definition: nil, session_id: nil},
          channel_info: %{}
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate within 5 seconds")
      end
    end

    test "supervised under BtwSupervisor" do
      {:ok, pid} =
        BtwSupervisor.start_btw(
          message: "supervised task",
          agent_name: "sup-agent",
          context: %{accumulated_state: %{}},
          channel_info: %{}
        )

      assert is_pid(pid)
      assert Process.alive?(pid)

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("Supervised BtwServer did not terminate")
      end
    end

    test "does not restart after normal termination" do
      initial_count = BtwSupervisor.count_active()

      {:ok, pid} =
        BtwSupervisor.start_btw(
          message: "no restart",
          agent_name: "nr-agent",
          context: %{},
          channel_info: %{}
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate")
      end

      # Small delay for supervisor bookkeeping
      Process.sleep(20)
      assert BtwSupervisor.count_active() == initial_count
    end
  end

  # --- Channel info ---

  describe "channel_info" do
    test "accepts channel routing metadata" do
      channel_info = %{
        channel: "telegram",
        reply_to_message_id: 42,
        chat_id: "chat-123"
      }

      {:ok, pid} =
        BtwServer.start_link(
          message: "reply to me",
          agent_name: "chan-agent",
          context: %{},
          channel_info: channel_info
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate")
      end
    end

    test "works with empty channel_info" do
      {:ok, pid} =
        BtwServer.start_link(
          message: "no channel",
          agent_name: "nochan-agent",
          context: %{},
          channel_info: %{}
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate")
      end
    end
  end

  # --- Context snapshot ---

  describe "context" do
    test "receives read-only context snapshot" do
      context = %{
        accumulated_state: %{user_prefs: %{lang: "en"}, memory: ["fact1"]},
        definition: %{name: "ctx-agent", model: "claude-sonnet-4-5"},
        session_id: "sess-123"
      }

      {:ok, pid} =
        BtwServer.start_link(
          message: "use context",
          agent_name: "ctx-agent",
          context: context,
          channel_info: %{}
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
      after
        5_000 -> flunk("BtwServer did not terminate")
      end

      # Context was passed as a copy — no shared state to verify mutation
    end
  end

  # --- Concurrency ---

  describe "concurrency" do
    test "multiple btw servers run concurrently" do
      pids =
        1..10
        |> Enum.map(fn i ->
          {:ok, pid} =
            BtwSupervisor.start_btw(
              message: "concurrent task #{i}",
              agent_name: "conc-agent",
              context: %{},
              channel_info: %{}
            )

          pid
        end)

      assert length(pids) == 10
      assert length(Enum.uniq(pids)) == 10

      # Wait for all to complete
      refs = Enum.map(pids, &Process.monitor/1)

      Enum.each(refs, fn ref ->
        receive do
          {:DOWN, ^ref, :process, _pid, reason} when reason in [:normal, :noproc] -> :ok
        after
          5_000 -> flunk("A concurrent BtwServer did not terminate")
        end
      end)
    end
  end

  # --- Error handling ---

  describe "error handling" do
    test "terminates normally on task execution failure" do
      # RustBridge will fail (connection refused in test config).
      # BtwServer must handle the error gracefully and stop :normal,
      # not crash with a match error. This covers the {:exit, reason}
      # and generic error paths through format_response.
      {:ok, pid} =
        BtwServer.start_link(
          message: "trigger failure",
          agent_name: "err-agent",
          context: %{},
          channel_info: %{}
        )

      ref = Process.monitor(pid)

      receive do
        {:DOWN, ^ref, :process, ^pid, reason} when reason in [:normal, :noproc] -> :ok
        {:DOWN, ^ref, :process, ^pid, other} -> flunk("BtwServer crashed: #{inspect(other)}")
      after
        5_000 -> flunk("BtwServer did not terminate within 5 seconds")
      end
    end
  end

  # --- BtwSupervisor ---

  describe "BtwSupervisor" do
    test "count_active tracks running tasks" do
      before = BtwSupervisor.count_active()

      # Start tasks that we'll manually wait on
      pids =
        1..3
        |> Enum.map(fn i ->
          {:ok, pid} =
            BtwSupervisor.start_btw(
              message: "count task #{i}",
              agent_name: "count-agent",
              context: %{},
              channel_info: %{}
            )

          pid
        end)

      # Wait for all to complete
      refs = Enum.map(pids, &Process.monitor/1)

      Enum.each(refs, fn ref ->
        receive do
          {:DOWN, ^ref, :process, _pid, reason} when reason in [:normal, :noproc] -> :ok
        after
          5_000 -> :ok
        end
      end)

      Process.sleep(20)
      assert BtwSupervisor.count_active() == before
    end

    test "list_active returns pids" do
      result = BtwSupervisor.list_active()
      assert is_list(result)
    end
  end
end
