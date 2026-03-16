defmodule RustyclawOrchestrator.Plugins.RetrySchedulerTest do
  use ExUnit.Case, async: true

  alias RustyclawOrchestrator.Plugins.RetryScheduler

  defmodule MockManager do
    @moduledoc false
    use GenServer

    def start_link(opts \\ []) do
      {plugins, opts} = Keyword.pop(opts, :plugins, [])
      GenServer.start_link(__MODULE__, plugins, opts)
    end

    @impl true
    def init(plugins), do: {:ok, plugins}

    @impl true
    def handle_call({:plugins_for_capabilities, _caps}, _from, plugins) do
      {:reply, plugins, plugins}
    end

    def handle_call(:list_plugins, _from, plugins) do
      {:reply, plugins, plugins}
    end
  end

  setup do
    name = :"retry_scheduler_#{:erlang.unique_integer([:positive])}"
    manager_name = :"mock_manager_#{:erlang.unique_integer([:positive])}"

    {:ok, _manager} =
      start_supervised(
        {MockManager, name: manager_name, plugins: []},
        id: manager_name
      )

    {:ok, _pid} =
      start_supervised(
        {RetryScheduler, name: name, manager: manager_name},
        id: name
      )

    %{server: name, manager: manager_name}
  end

  describe "schedule_retry/4" do
    test "schedules a transient retry with backoff", %{server: server} do
      test_pid = self()
      callback = fn event -> send(test_pid, {:callback, event}) end

      task = %{id: "task-1", description: "test", capabilities: [:coding]}

      assert :ok =
               RetryScheduler.schedule_retry(task, :api_error, "claude_code",
                 server: server,
                 callback: callback
               )

      # Should fire within ~1s (first retry backoff)
      assert_receive {:callback, {:retry, retried_task, "claude_code"}}, 2_000
      assert retried_task.retry_attempt == 1
    end

    test "exponential backoff increases delay", %{server: server} do
      test_pid = self()
      callback = fn event -> send(test_pid, {:callback, event}) end

      # First retry: ~1s
      task1 = %{id: "task-exp-1", description: "test", capabilities: [:coding]}

      RetryScheduler.schedule_retry(task1, :api_error, "claude_code",
        server: server,
        callback: callback
      )

      assert_receive {:callback, {:retry, _, _}}, 2_000

      # Second retry: ~2s (retry_attempt already 1 from first)
      task2 = %{
        id: "task-exp-2",
        description: "test",
        capabilities: [:coding],
        retry_attempt: 1
      }

      RetryScheduler.schedule_retry(task2, :api_error, "claude_code",
        server: server,
        callback: callback
      )

      assert_receive {:callback, {:retry, retried, _}}, 3_000
      assert retried.retry_attempt == 2
    end

    test "rate limited tasks use retry_after delay", %{server: server} do
      test_pid = self()
      callback = fn event -> send(test_pid, {:callback, event}) end

      task = %{id: "task-rl", description: "test", capabilities: [:coding]}

      RetryScheduler.schedule_retry(task, {:rate_limited, 1}, "claude_code",
        server: server,
        callback: callback
      )

      # Should fire after ~1 second (retry_after = 1s)
      assert_receive {:callback, {:retry, _, "claude_code"}}, 2_000
    end

    test "exhausts retries after max attempts", %{server: server} do
      test_pid = self()
      callback = fn event -> send(test_pid, {:callback, event}) end

      # retry_attempt 5 means next would be attempt 6 > max 5
      task = %{
        id: "task-exhaust",
        description: "test",
        capabilities: [:coding],
        retry_attempt: 5
      }

      assert {:error, :retries_exhausted} =
               RetryScheduler.schedule_retry(task, :api_error, "claude_code",
                 server: server,
                 callback: callback
               )

      assert_receive {:callback, {:exhausted, _task}}, 500
    end

    test "falls back to alternative plugin on exhaustion", _ctx do
      test_pid = self()
      callback = fn event -> send(test_pid, {:callback, event}) end

      # Set up a mock manager that returns an alternative plugin
      alt_manager = :"alt_manager_#{:erlang.unique_integer([:positive])}"

      {:ok, _} =
        start_supervised(
          {MockManager,
           name: alt_manager,
           plugins: [
             %{
               name: "codex",
               module: :mock,
               state: %{},
               capabilities: [:coding],
               status: :healthy
             }
           ]},
          id: alt_manager
        )

      retry_name = :"retry_fallback_#{:erlang.unique_integer([:positive])}"

      {:ok, _} =
        start_supervised(
          {RetryScheduler, name: retry_name, manager: alt_manager},
          id: retry_name
        )

      task = %{
        id: "task-fallback",
        description: "test",
        capabilities: [:coding],
        retry_attempt: 5
      }

      assert :ok =
               RetryScheduler.schedule_retry(task, :api_error, "claude_code",
                 server: retry_name,
                 callback: callback
               )

      # Should fall back to codex
      assert_receive {:callback, {:retry, _task, "codex"}}, 500
    end
  end

  describe "pending_count/1" do
    test "tracks pending retries", %{server: server} do
      assert 0 == RetryScheduler.pending_count(server: server)

      task = %{id: "task-count", description: "test", capabilities: [:coding]}
      RetryScheduler.schedule_retry(task, :api_error, "claude_code", server: server)

      assert 1 == RetryScheduler.pending_count(server: server)

      # Wait for retry to fire and clear from pending
      Process.sleep(1_500)
      assert 0 == RetryScheduler.pending_count(server: server)
    end
  end

  describe "list_pending/1" do
    test "lists pending retry details", %{server: server} do
      task = %{id: "task-list", description: "test", capabilities: [:coding]}
      RetryScheduler.schedule_retry(task, :api_error, "claude_code", server: server)

      entries = RetryScheduler.list_pending(server: server)
      assert length(entries) == 1
      assert hd(entries).plugin_name == "claude_code"
      assert hd(entries).attempt == 1
    end
  end
end
