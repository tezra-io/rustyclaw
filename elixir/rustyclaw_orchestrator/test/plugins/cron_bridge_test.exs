defmodule RustyclawOrchestrator.Plugins.CronBridgeTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.{CronBridge, Manager}

  defmodule MockCodingPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(state, task, event_handler) do
      event_handler.({:chunk, "working on #{task[:description]}"})
      {:ok, {:complete, %{output: "completed: #{task[:description]}"}}, state}
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

  defmodule FailingPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(_state, _task, _handler), do: {:error, :api_error}

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
    # Start isolated manager and worker infrastructure
    manager_name = :"cron_bridge_manager_#{:erlang.unique_integer([:positive])}"
    task_sup = :"cron_bridge_task_sup_#{:erlang.unique_integer([:positive])}"
    worker_sup = :"cron_bridge_worker_sup_#{:erlang.unique_integer([:positive])}"

    start_supervised!({Manager, name: manager_name})
    start_supervised!({Task.Supervisor, name: task_sup})

    start_supervised!(
      {DynamicSupervisor,
       name: worker_sup, strategy: :one_for_one, max_restarts: 5, max_seconds: 5}
    )

    %{manager: manager_name, task_sup: task_sup, worker_sup: worker_sup}
  end

  describe "submit_coding_task/2" do
    test "submits issue to a matching plugin and returns result", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      issue = %{
        identifier: "TEZ-100",
        title: "Fix the bug",
        description: "Something is broken",
        repo_path: "/tmp/fake-repo"
      }

      result =
        CronBridge.submit_coding_task(issue,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert {:ok, %{output: output}} = result
      assert output =~ "TEZ-100"
    end

    test "returns error when no plugin available", ctx do
      issue = %{identifier: "TEZ-101", title: "No plugin", description: "None"}

      result =
        CronBridge.submit_coding_task(issue, manager: ctx.manager)

      assert {:error, :no_available_plugin} = result
    end

    test "returns error from failing plugin", ctx do
      Manager.start_plugin(
        %{name: "failing", module: FailingPlugin, config: %{}},
        server: ctx.manager
      )

      issue = %{
        identifier: "TEZ-102",
        title: "Will fail",
        description: "Error expected",
        repo_path: "/tmp/fake-repo"
      }

      result =
        CronBridge.submit_coding_task(issue,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert {:error, :api_error} = result
    end

    test "forwards events via event_handler", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      test_pid = self()
      handler = fn event -> send(test_pid, {:event, event}) end

      issue = %{
        identifier: "TEZ-103",
        title: "Event test",
        description: "Check events",
        repo_path: "/tmp/fake-repo"
      }

      CronBridge.submit_coding_task(issue,
        manager: ctx.manager,
        worker_supervisor: ctx.worker_sup,
        event_handler: handler
      )

      assert_received {:event, {:chunk, msg}}
      assert msg =~ "TEZ-103"
    end
  end

  describe "submit_batch/2" do
    test "processes multiple issues sequentially and returns results", ctx do
      Manager.start_plugin(
        %{name: "mock_coder", module: MockCodingPlugin, config: %{}},
        server: ctx.manager
      )

      issues = [
        %{
          identifier: "TEZ-200",
          title: "First",
          description: "First issue",
          repo_path: "/tmp/fake"
        },
        %{
          identifier: "TEZ-201",
          title: "Second",
          description: "Second issue",
          repo_path: "/tmp/fake"
        }
      ]

      results =
        CronBridge.submit_batch(issues,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert length(results) == 2
      assert {"TEZ-200", {:ok, _}} = Enum.at(results, 0)
      assert {"TEZ-201", {:ok, _}} = Enum.at(results, 1)
    end

    test "continues after failure in batch", ctx do
      # Register failing plugin first, then we'll swap to test that
      # batch doesn't abort on individual failure
      Manager.start_plugin(
        %{name: "failing", module: FailingPlugin, config: %{}},
        server: ctx.manager
      )

      issues = [
        %{
          identifier: "TEZ-300",
          title: "Will fail",
          description: "Error",
          repo_path: "/tmp/fake"
        },
        %{
          identifier: "TEZ-301",
          title: "Also fails",
          description: "Error too",
          repo_path: "/tmp/fake"
        }
      ]

      results =
        CronBridge.submit_batch(issues,
          manager: ctx.manager,
          worker_supervisor: ctx.worker_sup
        )

      assert length(results) == 2
      assert {"TEZ-300", {:error, :api_error}} = Enum.at(results, 0)
      assert {"TEZ-301", {:error, :api_error}} = Enum.at(results, 1)
    end

    test "returns empty list for empty batch", ctx do
      results = CronBridge.submit_batch([], manager: ctx.manager)
      assert results == []
    end
  end
end
