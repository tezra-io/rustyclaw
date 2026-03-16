defmodule RustyclawOrchestrator.Plugins.WorkerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.Worker

  defmodule CompletePlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(state, _task, event_handler) do
      event_handler.({:chunk, "working..."})
      {:ok, {:complete, %{output: "all done"}}, state}
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

  defmodule ToolUsePlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(state, task, event_handler) do
      iteration = Map.get(state, :iteration, 0)

      if iteration >= 1 or Map.has_key?(task, :tool_results) do
        event_handler.({:chunk, "complete"})
        {:ok, {:complete, %{output: "done after tools"}}, state}
      else
        event_handler.({:tool_use, "shell", %{cmd: "echo test"}})

        {:ok, {:tool_use, [%{name: "shell", args: %{cmd: "echo test"}}]},
         Map.put(state, :iteration, iteration + 1)}
      end
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

  defmodule InfiniteLoopPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(state, _task, _handler) do
      {:ok, {:tool_use, [%{name: "shell", args: %{cmd: "echo loop"}}]}, state}
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

  defmodule ErrorPlugin do
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

  defmodule RateLimitPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, config}

    @impl true
    def execute(_state, _task, _handler), do: {:error, {:rate_limited, 60}}

    @impl true
    def health(_state), do: :healthy
    @impl true
    def capabilities, do: [:coding]
    @impl true
    def rate_limit_status(_state), do: %{remaining: 0, reset_at: nil, limited: true}
    @impl true
    def disconnect(_state), do: :ok
  end

  setup do
    task_sup = :"worker_task_sup_#{:erlang.unique_integer([:positive])}"
    start_supervised!({Task.Supervisor, name: task_sup})
    %{task_supervisor: task_sup}
  end

  describe "complete flow" do
    test "executes and returns complete result", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      result =
        Worker.execute(worker,
          plugin_module: CompletePlugin,
          plugin_state: %{},
          task: %{description: "test task"},
          task_supervisor: task_sup
        )

      assert {:ok, %{output: "all done"}} = result
    end

    test "calls event handler during execution", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})
      test_pid = self()
      handler = fn event -> send(test_pid, {:event, event}) end

      Worker.execute(worker,
        plugin_module: CompletePlugin,
        plugin_state: %{},
        task: %{description: "test"},
        event_handler: handler,
        task_supervisor: task_sup
      )

      assert_received {:event, {:chunk, "working..."}}
    end
  end

  describe "tool_use loop" do
    test "loops through tool_use then completes", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      result =
        Worker.execute(worker,
          plugin_module: ToolUsePlugin,
          plugin_state: %{},
          task: %{description: "tool task"},
          task_supervisor: task_sup
        )

      assert {:ok, %{output: "done after tools"}} = result
    end
  end

  describe "max iterations guard" do
    test "stops after max iterations", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      result =
        Worker.execute(worker,
          plugin_module: InfiniteLoopPlugin,
          plugin_state: %{},
          task: %{description: "loop task"},
          max_iterations: 3,
          task_supervisor: task_sup
        )

      assert {:error, {:max_iterations, 3}} = result
    end
  end

  describe "error handling" do
    test "returns error from plugin", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      result =
        Worker.execute(worker,
          plugin_module: ErrorPlugin,
          plugin_state: %{},
          task: %{description: "error task"},
          task_supervisor: task_sup
        )

      assert {:error, :api_error} = result
    end

    test "returns rate limit error", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      result =
        Worker.execute(worker,
          plugin_module: RateLimitPlugin,
          plugin_state: %{},
          task: %{description: "rate limit task"},
          task_supervisor: task_sup
        )

      assert {:error, {:rate_limited, 60}} = result
    end

    test "rejects concurrent execution", %{task_supervisor: task_sup} do
      {:ok, worker} = start_supervised({Worker, task_supervisor: task_sup})

      # Use a plugin that blocks in execute/3 so the worker stays busy
      defmodule BlockingPlugin do
        @behaviour RustyclawOrchestrator.Plugins.Behaviour
        @impl true
        def connect(config), do: {:ok, config}

        @impl true
        def execute(_state, _task, _handler) do
          # Block long enough for the concurrent test
          Process.sleep(2_000)
          {:ok, {:complete, :done}, %{}}
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

      # Start a long-running task
      spawn(fn ->
        Worker.execute(worker,
          plugin_module: BlockingPlugin,
          plugin_state: %{},
          task: %{description: "blocking"},
          task_supervisor: task_sup
        )
      end)

      # Give it time to enter the execute call
      Process.sleep(100)

      result =
        Worker.execute(worker,
          plugin_module: CompletePlugin,
          plugin_state: %{},
          task: %{description: "second"},
          task_supervisor: task_sup
        )

      assert {:error, :busy} = result
    end
  end
end
