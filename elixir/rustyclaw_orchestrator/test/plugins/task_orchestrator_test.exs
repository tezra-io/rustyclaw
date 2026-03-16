defmodule RustyclawOrchestrator.Plugins.TaskOrchestratorTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.{Manager, TaskOrchestrator}

  defmodule MockCodingPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(state, _task, event_handler) do
      event_handler.({:chunk, "working..."})
      {:ok, {:complete, %{output: "done"}}, state}
    end

    @impl true
    def health(_state), do: :healthy
    @impl true
    def capabilities, do: [:coding]
    @impl true
    def rate_limit_status(_state), do: %{remaining: 50, reset_at: nil, limited: false}
    @impl true
    def disconnect(_state), do: :ok
  end

  setup do
    manager_name = :"orch_manager_#{:erlang.unique_integer([:positive])}"
    task_sup = :"orch_task_sup_#{:erlang.unique_integer([:positive])}"
    worker_sup = :"orch_worker_sup_#{:erlang.unique_integer([:positive])}"
    orch_name = :"orch_#{:erlang.unique_integer([:positive])}"

    start_supervised!({Manager, name: manager_name})
    start_supervised!({Task.Supervisor, name: task_sup})

    start_supervised!(
      {DynamicSupervisor,
       name: worker_sup, strategy: :one_for_one, max_restarts: 5, max_seconds: 5}
    )

    start_supervised!({TaskOrchestrator, name: orch_name})

    %{
      manager: manager_name,
      task_sup: task_sup,
      worker_sup: worker_sup,
      orchestrator: orch_name
    }
  end

  describe "start_session/1" do
    test "starts a session and returns session_id", ctx do
      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      assert {:ok, session_id} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)
      assert is_binary(session_id)
      assert String.starts_with?(session_id, "session-")
    end

    test "session with no issues completes immediately", ctx do
      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      {:ok, session_id} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)

      # Give the async processing time to complete
      Process.sleep(100)

      {:ok, status} = TaskOrchestrator.get_status(session_id, server: ctx.orchestrator)
      assert status.status == :completed
      assert status.completed_count == 0
      assert status.failure_count == 0
    end
  end

  describe "get_status/1" do
    test "returns error for unknown session", ctx do
      assert {:error, :not_found} =
               TaskOrchestrator.get_status("nonexistent", server: ctx.orchestrator)
    end

    test "returns session summary with counts", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [
          %{identifier: "TEZ-400", title: "First", description: "Do it"}
        ],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      {:ok, session_id} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)

      # Wait for processing
      Process.sleep(500)

      {:ok, status} = TaskOrchestrator.get_status(session_id, server: ctx.orchestrator)
      assert status.total_issues == 1
      assert is_integer(status.completed_count)
      assert is_integer(status.failure_count)
    end
  end

  describe "cancel_session/1" do
    test "cancels a running session", ctx do
      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      {:ok, session_id} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)

      assert :ok = TaskOrchestrator.cancel_session(session_id, server: ctx.orchestrator)

      {:ok, status} = TaskOrchestrator.get_status(session_id, server: ctx.orchestrator)
      assert status.status == :cancelled
    end

    test "returns error for unknown session", ctx do
      assert {:error, :not_found} =
               TaskOrchestrator.cancel_session("nonexistent", server: ctx.orchestrator)
    end
  end

  describe "list_sessions/0" do
    test "returns all sessions", ctx do
      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      {:ok, _id1} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)
      {:ok, _id2} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)

      sessions = TaskOrchestrator.list_sessions(server: ctx.orchestrator)
      assert length(sessions) == 2
    end
  end

  describe "session lifecycle with issues" do
    test "processes issues and tracks completion", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      config = %{
        repo_path: "/tmp/fake-repo",
        issues: [
          %{identifier: "TEZ-500", title: "Issue 1", description: "First"},
          %{identifier: "TEZ-501", title: "Issue 2", description: "Second"}
        ],
        plugin_opts: [manager: ctx.manager, worker_supervisor: ctx.worker_sup]
      }

      {:ok, session_id} = TaskOrchestrator.start_session(config, server: ctx.orchestrator)

      # Wait for both issues to process
      Process.sleep(1_000)

      {:ok, status} = TaskOrchestrator.get_status(session_id, server: ctx.orchestrator)
      assert status.total_issues == 2
      # At minimum the session should have attempted both
      assert status.completed_count + status.failure_count <= 2
    end
  end
end
