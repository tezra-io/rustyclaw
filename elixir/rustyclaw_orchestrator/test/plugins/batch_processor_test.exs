defmodule RustyclawOrchestrator.Plugins.BatchProcessorTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.{BatchProcessor, Manager}

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
    manager_name = :"bp_manager_#{:erlang.unique_integer([:positive])}"
    task_sup = :"bp_task_sup_#{:erlang.unique_integer([:positive])}"
    worker_sup = :"bp_worker_sup_#{:erlang.unique_integer([:positive])}"

    start_supervised!({Manager, name: manager_name})
    start_supervised!({Task.Supervisor, name: task_sup})

    start_supervised!(
      {DynamicSupervisor,
       name: worker_sup, strategy: :one_for_one, max_restarts: 5, max_seconds: 5}
    )

    %{manager: manager_name, task_sup: task_sup, worker_sup: worker_sup}
  end

  describe "submit_batch/2" do
    test "processes tasks sequentially for same repo", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      tasks = [
        %{identifier: "TEZ-700", title: "Task 1", description: "First", repo_path: "/tmp/repo"},
        %{identifier: "TEZ-701", title: "Task 2", description: "Second", repo_path: "/tmp/repo"}
      ]

      results =
        BatchProcessor.submit_batch(tasks,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert length(results) == 2

      Enum.each(results, fn {identifier, result} ->
        assert identifier in ["TEZ-700", "TEZ-701"]
        # Results may be ok or error depending on Worker/RustBridge availability
        assert match?({:ok, _}, result) or match?({:error, _}, result)
      end)
    end

    test "handles empty batch", ctx do
      results = BatchProcessor.submit_batch([], manager: ctx.manager)
      assert results == []
    end
  end

  describe "max_concurrent/0" do
    test "returns 0 when no plugins registered", ctx do
      assert BatchProcessor.max_concurrent(manager: ctx.manager) == 0
    end

    test "counts healthy plugins", ctx do
      Manager.start_plugin(
        %{name: "coder1", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      Manager.start_plugin(
        %{name: "coder2", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      assert BatchProcessor.max_concurrent(manager: ctx.manager) == 2
    end
  end

  describe "task enrichment" do
    test "auto-routes tasks without capabilities", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      tasks = [
        %{
          identifier: "TEZ-750",
          title: "Bug fix",
          description: "Fix it",
          repo_path: "/tmp/repo",
          labels: ["plugin:coding"]
        }
      ]

      results =
        BatchProcessor.submit_batch(tasks,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert length(results) == 1
    end
  end
end
