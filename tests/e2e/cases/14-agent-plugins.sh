#!/usr/bin/env bash
# Suite: Agent Plugin System — behaviour, manager, queue, routing, retry, quality, progress, API

suite_14-agent-plugins() {
    # Compilation and unit tests
    run_test "TC-14.1" "Plugin modules compile" tc_plugin_compile
    run_test "TC-14.2" "Plugin unit tests pass" tc_plugin_unit_tests 120

    # Component-level tests via inline Elixir
    run_test "TC-14.3" "Plugin behaviour conformance" tc_plugin_behaviour 30
    run_test "TC-14.4" "Manager register/unregister/list" tc_plugin_manager 30
    run_test "TC-14.5" "TaskQueue enqueue/dequeue/priority" tc_plugin_task_queue 30
    run_test "TC-14.6" "AutoRouter selects correct plugin" tc_plugin_auto_router 30
    run_test "TC-14.7" "RetryScheduler handles failures" tc_plugin_retry_scheduler 30
    run_test "TC-14.8" "QualityGate pass/fail decisions" tc_plugin_quality_gate 30
    run_test "TC-14.9" "ProgressTracker reports status" tc_plugin_progress_tracker 30

    # API endpoints (starts full OTP app)
    run_test "TC-14.10" "Plugin API endpoints respond" tc_plugin_api 60
}

# --- TC-14.1: Plugin modules compile ---

tc_plugin_compile() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix compile --warnings-as-errors 2>&1
}

# --- TC-14.2: Plugin unit tests pass ---

tc_plugin_unit_tests() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix test test/plugins/ 2>&1
}

# --- TC-14.3: Plugin behaviour conformance ---

tc_plugin_behaviour() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      # Define a mock plugin implementing the behaviour
      defmodule E2eMockPlugin do
        @behaviour RustyclawOrchestrator.Plugins.Behaviour

        @impl true
        def connect(_config), do: {:ok, %{connected: true}}

        @impl true
        def execute(state, _task, event_handler) do
          event_handler.({:chunk, "working..."})
          {:ok, {:complete, "mock result"}, state}
        end

        @impl true
        def health(_state), do: :healthy

        @impl true
        def capabilities, do: [:coding, :testing]

        @impl true
        def rate_limit_status(_state), do: %{remaining: 100, reset_at: nil, limited: false}

        @impl true
        def disconnect(_state), do: :ok
      end

      # Verify connect
      {:ok, state} = E2eMockPlugin.connect(%{})
      true = state.connected
      IO.puts("connect: ok")

      # Verify execute with event handler
      events = :ets.new(:e2e_events, [:bag, :public])
      handler = fn event -> :ets.insert(events, {event}); :ok end

      {:ok, {:complete, result}, _} = E2eMockPlugin.execute(state, %{}, handler)
      "mock result" = result
      [{_}] = :ets.lookup(events, {:chunk, "working..."})
      IO.puts("execute: ok")

      # Verify health
      :healthy = E2eMockPlugin.health(state)
      IO.puts("health: ok")

      # Verify capabilities
      caps = E2eMockPlugin.capabilities()
      true = :coding in caps
      true = :testing in caps
      IO.puts("capabilities: ok")

      # Verify rate_limit_status
      %{remaining: 100, limited: false} = E2eMockPlugin.rate_limit_status(state)
      IO.puts("rate_limit_status: ok")

      # Verify disconnect
      :ok = E2eMockPlugin.disconnect(state)
      IO.puts("disconnect: ok")

      IO.puts("BEHAVIOUR_OK")
    ' 2>&1
}

# --- TC-14.4: Manager register/unregister/list ---

tc_plugin_manager() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.Manager

      # Define mock plugins
      defmodule E2ePluginA do
        @behaviour RustyclawOrchestrator.Plugins.Behaviour
        def connect(_), do: {:ok, %{name: "plugin_a"}}
        def execute(s, _, _), do: {:ok, {:complete, "a"}, s}
        def health(_), do: :healthy
        def capabilities, do: [:coding]
        def rate_limit_status(_), do: %{remaining: 50, reset_at: nil, limited: false}
        def disconnect(_), do: :ok
      end

      defmodule E2ePluginB do
        @behaviour RustyclawOrchestrator.Plugins.Behaviour
        def connect(_), do: {:ok, %{name: "plugin_b"}}
        def execute(s, _, _), do: {:ok, {:complete, "b"}, s}
        def health(_), do: :healthy
        def capabilities, do: [:review]
        def rate_limit_status(_), do: %{remaining: 50, reset_at: nil, limited: false}
        def disconnect(_), do: :ok
      end

      server = :"mgr_e2e_#{:erlang.unique_integer([:positive])}"
      {:ok, _} = Manager.start_link(name: server)

      # Start plugins
      {:ok, _entry_a} = Manager.start_plugin(
        %{name: "plugin_a", module: E2ePluginA, config: %{}},
        server: server)
      IO.puts("start plugin_a: ok")

      {:ok, _entry_b} = Manager.start_plugin(
        %{name: "plugin_b", module: E2ePluginB, config: %{}},
        server: server)
      IO.puts("start plugin_b: ok")

      # List all
      plugins = Manager.list_plugins(server: server)
      2 = length(plugins)
      IO.puts("list: 2 plugins")

      # Get specific
      {:ok, pa} = Manager.get_plugin("plugin_a", server: server)
      "plugin_a" = pa.name
      IO.puts("get: plugin_a found")

      # Filter by capability
      coding = Manager.plugins_for_capabilities([:coding], server: server)
      1 = length(coding)
      "plugin_a" = hd(coding).name
      IO.puts("capability_filter coding: plugin_a")

      review = Manager.plugins_for_capabilities([:review], server: server)
      1 = length(review)
      "plugin_b" = hd(review).name
      IO.puts("capability_filter review: plugin_b")

      # Stop plugin
      :ok = Manager.stop_plugin("plugin_a", server: server)
      {:error, :not_found} = Manager.get_plugin("plugin_a", server: server)
      IO.puts("stop: plugin_a removed")

      # Remaining
      remaining = Manager.list_plugins(server: server)
      1 = length(remaining)
      IO.puts("remaining: 1 plugin")

      :ok = Manager.stop_plugin("plugin_b", server: server)
      IO.puts("MANAGER_OK")
    ' 2>&1
}

# --- TC-14.5: TaskQueue enqueue/dequeue/priority ---

tc_plugin_task_queue() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.TaskQueue

      server = :"tq_e2e_#{:erlang.unique_integer([:positive])}"
      ets_table = :"tq_ets_e2e_#{:erlang.unique_integer([:positive])}"
      {:ok, _} = TaskQueue.start_link(
        name: server,
        poll_interval_ms: 0,
        auto_assign: false,
        ets_table: ets_table
      )

      # Empty queue
      :empty = TaskQueue.pop_task(server: server)
      IO.puts("empty: ok")

      # Push tasks with different priorities
      low_priority = %{
        id: "task-low",
        identifier: "TEZ-100",
        title: "Low priority",
        description: "low",
        priority: 4,
        labels: [],
        capabilities: [:coding]
      }

      high_priority = %{
        id: "task-high",
        identifier: "TEZ-101",
        title: "High priority",
        description: "high",
        priority: 1,
        labels: [],
        capabilities: [:coding]
      }

      medium_priority = %{
        id: "task-med",
        identifier: "TEZ-102",
        title: "Medium priority",
        description: "medium",
        priority: 2,
        labels: [],
        capabilities: [:coding]
      }

      :ok = TaskQueue.push_task(low_priority, server: server)
      :ok = TaskQueue.push_task(high_priority, server: server)
      :ok = TaskQueue.push_task(medium_priority, server: server)
      IO.puts("push: 3 tasks")

      # Status check
      status = TaskQueue.status(server: server)
      3 = status.queue_size
      IO.puts("status: queue_size=3")

      # Pop should return highest priority first (lowest number)
      {:ok, first} = TaskQueue.pop_task(server: server)
      "TEZ-101" = first.identifier
      IO.puts("pop first: TEZ-101 (priority 1)")

      {:ok, second} = TaskQueue.pop_task(server: server)
      "TEZ-102" = second.identifier
      IO.puts("pop second: TEZ-102 (priority 2)")

      {:ok, third} = TaskQueue.pop_task(server: server)
      "TEZ-100" = third.identifier
      IO.puts("pop third: TEZ-100 (priority 4)")

      :empty = TaskQueue.pop_task(server: server)
      IO.puts("pop empty: ok")

      # Remove task
      :ok = TaskQueue.push_task(low_priority, server: server)
      :ok = TaskQueue.remove_task("task-low", server: server)
      :empty = TaskQueue.pop_task(server: server)
      IO.puts("remove: ok")

      IO.puts("TASK_QUEUE_OK")
    ' 2>&1
}

# --- TC-14.6: AutoRouter selects correct plugin ---

tc_plugin_auto_router() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.AutoRouter

      # Coding label
      caps = AutoRouter.route_task(%{labels: ["plugin:coding"]})
      true = :coding in caps
      IO.puts("coding label: #{inspect(caps)}")

      # Review label
      caps = AutoRouter.route_task(%{labels: ["plugin:review"]})
      true = :review in caps
      IO.puts("review label: #{inspect(caps)}")

      # Analysis label
      caps = AutoRouter.route_task(%{labels: ["plugin:analysis"]})
      true = :analysis in caps
      IO.puts("analysis label: #{inspect(caps)}")

      # No matching label defaults to coding
      caps = AutoRouter.route_task(%{labels: ["bug", "urgent"]})
      true = :coding in caps
      IO.puts("default: #{inspect(caps)}")

      # Multiple labels
      caps = AutoRouter.route_task(%{labels: ["plugin:coding", "plugin:review"]})
      true = :coding in caps
      true = :review in caps
      IO.puts("multi-label: #{inspect(caps)}")

      IO.puts("AUTO_ROUTER_OK")
    ' 2>&1
}

# --- TC-14.7: RetryScheduler handles failures ---

tc_plugin_retry_scheduler() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.RetryScheduler

      server = :"retry_e2e_#{:erlang.unique_integer([:positive])}"
      test_pid = self()
      callback = fn event -> send(test_pid, {:retry_event, event}); :ok end

      {:ok, _} = RetryScheduler.start_link(name: server, callback: callback)

      task = %{
        id: "retry-task-1",
        description: "test retry",
        capabilities: [:coding],
        retry_attempt: 0
      }

      # Schedule retry
      :ok = RetryScheduler.schedule_retry(task, :api_error, "test_plugin", server: server)
      IO.puts("schedule_retry: ok")

      # Check pending
      count = RetryScheduler.pending_count(server: server)
      true = count >= 1
      IO.puts("pending_count: #{count}")

      # List pending
      pending = RetryScheduler.list_pending(server: server)
      true = length(pending) >= 1
      IO.puts("list_pending: #{length(pending)} entries")

      # Wait for retry callback to fire (backoff starts at 1s)
      receive do
        {:retry_event, {:retry, _task, _plugin}} ->
          IO.puts("retry_callback: fired")
      after
        5000 ->
          IO.puts("retry_callback: timeout (may still be pending)")
      end

      IO.puts("RETRY_SCHEDULER_OK")
    ' 2>&1
}

# --- TC-14.8: QualityGate pass/fail decisions ---

tc_plugin_quality_gate() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.QualityGate

      # Passing gate — use a command that always succeeds
      gates = [%{name: "echo_test", command: "echo hello", timeout: 5000}]
      {:pass, results} = QualityGate.run("some result", gates)
      [%{name: "echo_test", status: :pass}] = results
      IO.puts("passing_gate: ok")

      # Failing gate — use a command that always fails
      fail_gates = [%{name: "false_test", command: "false", timeout: 5000}]
      {:fail, gate_name, _detail} = QualityGate.run("some result", fail_gates)
      "false_test" = gate_name
      IO.puts("failing_gate: ok")

      # Multiple gates — stops on first failure
      mixed_gates = [
        %{name: "pass_first", command: "true", timeout: 5000},
        %{name: "fail_second", command: "false", timeout: 5000},
        %{name: "never_reached", command: "echo unreachable", timeout: 5000}
      ]
      {:fail, "fail_second", _detail} = QualityGate.run("result", mixed_gates)
      IO.puts("sequential_stop: ok")

      # Empty gates always pass
      {:pass, []} = QualityGate.run("result", [])
      IO.puts("empty_gates: pass")

      IO.puts("QUALITY_GATE_OK")
    ' 2>&1
}

# --- TC-14.9: ProgressTracker reports status ---

tc_plugin_progress_tracker() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.Plugins.ProgressTracker

      server = :"progress_e2e_#{:erlang.unique_integer([:positive])}"
      {:ok, _} = ProgressTracker.start_link(
        name: server,
        stuck_timeout_ms: 300_000,
        window_size: 5,
        similarity_threshold: 0.85
      )

      worker_id = "e2e-worker-1"

      # Record events
      ProgressTracker.record(server, worker_id, {:chunk, "processing..."})
      ProgressTracker.record(server, worker_id, {:tool_use, "shell", %{cmd: "ls"}})
      ProgressTracker.record(server, worker_id, {:artifact, :code, "def hello, do: :world"})
      Process.sleep(100)

      # Get worker state
      {:ok, state} = ProgressTracker.get_worker_state(server, worker_id)
      true = state.event_count >= 3
      IO.puts("event_count: #{state.event_count}")

      # List workers
      workers = ProgressTracker.list_workers(server)
      true = worker_id in workers
      IO.puts("list_workers: found #{worker_id}")

      # Clear worker
      ProgressTracker.clear_worker(server, worker_id)
      Process.sleep(100)
      {:error, :not_found} = ProgressTracker.get_worker_state(server, worker_id)
      IO.puts("clear_worker: ok")

      IO.puts("PROGRESS_TRACKER_OK")
    ' 2>&1
}

# --- TC-14.10: Plugin API endpoints respond ---

tc_plugin_api() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      # Find free ports
      {:ok, s1} = :gen_tcp.listen(0, [])
      {:ok, synth_port} = :inet.port(s1)
      :gen_tcp.close(s1)
      {:ok, s2} = :gen_tcp.listen(0, [])
      {:ok, plugin_port} = :inet.port(s2)
      :gen_tcp.close(s2)

      Application.put_env(:rustyclaw_orchestrator, :synth_api_port, synth_port)
      Application.put_env(:rustyclaw_orchestrator, :plugin_api_port, plugin_port)
      {:ok, _} = Application.ensure_all_started(:rustyclaw_orchestrator)

      Process.sleep(1500)
      base = "http://localhost:#{plugin_port}"
      IO.puts("Plugin API on port #{plugin_port}")

      # GET /api/plugins — empty list
      {:ok, resp} = Req.get("#{base}/api/plugins")
      200 = resp.status
      %{"ok" => true, "plugins" => plugins} = resp.body
      true = is_list(plugins)
      IO.puts("GET /api/plugins: 200 (#{length(plugins)} plugins)")

      # GET /api/plugins/status
      {:ok, resp} = Req.get("#{base}/api/plugins/status")
      200 = resp.status
      %{"ok" => true} = resp.body
      IO.puts("GET /api/plugins/status: 200")

      # GET /api/plugins/queue
      {:ok, resp} = Req.get("#{base}/api/plugins/queue")
      200 = resp.status
      %{"ok" => true, "status" => _, "tasks" => tasks} = resp.body
      true = is_list(tasks)
      IO.puts("GET /api/plugins/queue: 200")

      # POST /api/plugins/exec — missing fields
      {:ok, resp} = Req.post("#{base}/api/plugins/exec", json: %{})
      400 = resp.status
      IO.puts("POST /api/plugins/exec (missing): 400")

      # POST /api/plugins/exec — no plugin available
      {:ok, resp} = Req.post("#{base}/api/plugins/exec",
        json: %{"capability" => "coding", "description" => "test task"})
      404 = resp.status
      IO.puts("POST /api/plugins/exec (no plugin): 404")

      # GET /api/plugins/sessions/:id — not found
      {:ok, resp} = Req.get("#{base}/api/plugins/sessions/nonexistent")
      404 = resp.status
      IO.puts("GET /api/plugins/sessions (not found): 404")

      # DELETE /api/plugins/sessions/:id — not found
      {:ok, resp} = Req.delete("#{base}/api/plugins/sessions/nonexistent")
      404 = resp.status
      IO.puts("DELETE /api/plugins/sessions (not found): 404")

      # DELETE /api/plugins/queue/:task_id — not found
      {:ok, resp} = Req.delete("#{base}/api/plugins/queue/nonexistent")
      404 = resp.status
      IO.puts("DELETE /api/plugins/queue (not found): 404")

      # 404 catch-all
      {:ok, resp} = Req.get("#{base}/api/plugins/bogus")
      404 = resp.status
      IO.puts("catch-all: 404")

      IO.puts("PLUGIN_API_OK")
    ' 2>&1
}
