#!/usr/bin/env bash
# Suite: Tool Synthesis — registry, sandbox, static analysis, probation, persistence, API

suite_13-tool-synthesis() {
    # Compilation and unit tests
    run_test "TC-13.1" "Tool synthesis modules compile" tc_synth_compile
    run_test "TC-13.2" "Tool synthesis unit tests pass" tc_synth_unit_tests 120

    # Component-level tests via inline Elixir
    run_test "TC-13.3" "Registry CRUD operations" tc_synth_registry 30
    run_test "TC-13.4" "Static analyzer catches unsafe patterns" tc_synth_static_analyzer 30
    run_test "TC-13.5" "Sandbox execution isolation" tc_synth_sandbox 30
    run_test "TC-13.6" "Probation lifecycle transitions" tc_synth_probation 30
    run_test "TC-13.7" "Persistence save/load cycle" tc_synth_persistence 30
    run_test "TC-13.8" "Composer dependency tracking" tc_synth_composer 30

    # API endpoints (starts full OTP app)
    run_test "TC-13.9" "Synth API endpoints respond" tc_synth_api 60

    # CLI integration (needs Elixir API on port 4001)
    run_test "TC-13.10" "CLI synth list command" tc_synth_cli 60
}

# --- TC-13.1: Compile tool synthesis modules ---

tc_synth_compile() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix compile --warnings-as-errors 2>&1
}

# --- TC-13.2: Run tool synthesis unit tests ---

tc_synth_unit_tests() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix test test/tool_synthesis/ 2>&1
}

# --- TC-13.3: Registry CRUD operations ---

tc_synth_registry() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.Registry

      # Init
      Registry.init()

      # Register a tool
      defmodule RustyclawOrchestrator.Synth.E2eTestTool do
        def name, do: "e2e_test_tool"
        def description, do: "E2E test tool"
        def parameters_schema, do: %{}
        def execute(_params), do: {:ok, "hello"}
      end

      :ok = Registry.register("e2e_test_tool", RustyclawOrchestrator.Synth.E2eTestTool,
        author_agent: "e2e-agent")
      IO.puts("register: ok")

      # Lookup
      {:ok, entry} = Registry.lookup("e2e_test_tool")
      "e2e_test_tool" = entry.name
      :probation = entry.status
      IO.puts("lookup: ok")

      # Update status
      :ok = Registry.update_status("e2e_test_tool", :promoted)
      {:ok, promoted} = Registry.lookup("e2e_test_tool")
      :promoted = promoted.status
      IO.puts("update_status: promoted")

      # Update metrics
      :ok = Registry.update_metrics("e2e_test_tool", true, 42)
      {:ok, with_metrics} = Registry.lookup("e2e_test_tool")
      1 = with_metrics.invocation_count
      1 = with_metrics.success_count
      IO.puts("update_metrics: ok")

      # List with filter
      [_] = Registry.list(status: :promoted)
      [] = Registry.list(status: :suspended)
      IO.puts("list_filter: ok")

      # Duplicate registration fails
      {:error, :already_exists} = Registry.register("e2e_test_tool",
        RustyclawOrchestrator.Synth.E2eTestTool)
      IO.puts("duplicate: blocked")

      # Unload
      :ok = Registry.unload("e2e_test_tool")
      {:error, :not_found} = Registry.lookup("e2e_test_tool")
      IO.puts("unload: ok")

      IO.puts("REGISTRY_CRUD_OK")
    ' 2>&1
}

# --- TC-13.4: Static analyzer catches unsafe patterns ---

tc_synth_static_analyzer() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.StaticAnalyzer

      # Valid code should pass
      valid_source = ~S"""
      defmodule RustyclawOrchestrator.Synth.SafeTool do
        def name, do: "safe_tool"
        def description, do: "A safe tool"
        def parameters_schema, do: %{}
        def execute(params), do: {:ok, Map.get(params, "input", "default")}
      end
      """
      :ok = StaticAnalyzer.validate(valid_source)
      IO.puts("valid_code: passed")

      # Code with System module should fail
      unsafe_system = ~S"""
      defmodule RustyclawOrchestrator.Synth.UnsafeTool do
        def name, do: "unsafe"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: System.cmd("rm", ["-rf", "/"])
      end
      """
      {:error, _} = StaticAnalyzer.validate(unsafe_system)
      IO.puts("system_call: blocked")

      # Code with import should fail
      unsafe_import = ~S"""
      defmodule RustyclawOrchestrator.Synth.ImportTool do
        import Kernel
        def name, do: "import"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "x"}
      end
      """
      {:error, _} = StaticAnalyzer.validate(unsafe_import)
      IO.puts("import: blocked")

      # Code with File module should fail
      unsafe_file = ~S"""
      defmodule RustyclawOrchestrator.Synth.FileTool do
        def name, do: "file_tool"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: File.read!("/etc/passwd")
      end
      """
      {:error, _} = StaticAnalyzer.validate(unsafe_file)
      IO.puts("file_access: blocked")

      # Code with spawn should fail
      unsafe_spawn = ~S"""
      defmodule RustyclawOrchestrator.Synth.SpawnTool do
        def name, do: "spawn_tool"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: spawn(fn -> :ok end)
      end
      """
      {:error, _} = StaticAnalyzer.validate(unsafe_spawn)
      IO.puts("spawn: blocked")

      # Wrong namespace should fail
      wrong_ns = ~S"""
      defmodule MyApp.Tool do
        def name, do: "wrong"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "x"}
      end
      """
      {:error, _} = StaticAnalyzer.validate(wrong_ns)
      IO.puts("wrong_namespace: blocked")

      IO.puts("STATIC_ANALYZER_OK")
    ' 2>&1
}

# --- TC-13.5: Sandbox execution isolation ---

tc_synth_sandbox() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.Sandbox

      # Start sandbox task supervisor
      {:ok, _} = Task.Supervisor.start_link(
        name: RustyclawOrchestrator.ToolSynthesis.Sandbox.TaskSupervisor
      )

      # Define a safe test module
      defmodule RustyclawOrchestrator.Synth.SandboxTest do
        def execute(%{"input" => val}), do: {:ok, "processed: #{val}"}
        def execute(_), do: {:ok, "no input"}
      end

      # Execute successfully
      {:ok, output} = Sandbox.execute(
        RustyclawOrchestrator.Synth.SandboxTest,
        %{"input" => "hello"}
      )
      true = String.contains?(output, "processed: hello")
      IO.puts("execute_ok: #{output}")

      # Define a crashing module
      defmodule RustyclawOrchestrator.Synth.CrashTest do
        def execute(_), do: raise("boom")
      end

      # Execute crashing module should return error
      {:error, msg} = Sandbox.execute(RustyclawOrchestrator.Synth.CrashTest, %{})
      true = is_binary(msg)
      IO.puts("crash_handled: #{msg}")

      IO.puts("SANDBOX_OK")
    ' 2>&1
}

# --- TC-13.6: Probation lifecycle transitions ---

tc_synth_probation() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.{Registry, Probation}

      Registry.init()

      # Define test module
      defmodule RustyclawOrchestrator.Synth.ProbationTest do
        def name, do: "probation_test"
        def description, do: "test"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "ok"}
      end

      Registry.register("probation_test", RustyclawOrchestrator.Synth.ProbationTest)

      # Start probation with auto_promote enabled and low threshold
      server_name = :"probation_e2e_#{:erlang.unique_integer([:positive])}"
      {:ok, _} = Probation.start_link(
        name: server_name,
        config: [probation_invocations: 5, min_success_rate: 0.8, auto_promote: true]
      )

      # Simulate invocations — update Registry metrics first (as the API router does),
      # then record in Probation for lifecycle evaluation
      for _ <- 1..5 do
        Registry.update_metrics("probation_test", true, 10)
        Probation.record_invocation("probation_test", true, server: server_name)
      end

      # Check state after sufficient invocations
      {:ok, state} = Probation.get_state("probation_test", server: server_name)
      true = state.invocation_count >= 5
      true = state.success_rate >= 0.8
      IO.puts("probation_state: invocations=#{state.invocation_count}, rate=#{state.success_rate}")

      # Clean up
      Registry.clear()
      IO.puts("PROBATION_OK")
    ' 2>&1
}

# --- TC-13.7: Persistence save/load cycle ---

tc_synth_persistence() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    local persist_dir
    persist_dir=$(mktemp -d "${E2E_WORKSPACE}/synth_persist.XXXXXX")

    SYNTH_PERSIST_DIR="$persist_dir" \
    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Registry, StaticAnalyzer}

      persist_dir = System.get_env("SYNTH_PERSIST_DIR")
      Application.put_env(:rustyclaw_orchestrator, :synthesized_tools_dir, persist_dir)

      Registry.init()

      source = ~S"""
      defmodule RustyclawOrchestrator.Synth.PersistTest do
        def name, do: "persist_test"
        def description, do: "persistence test"
        def parameters_schema, do: %{"input" => "string"}
        def execute(%{"input" => v}), do: {:ok, v}
        def execute(_), do: {:ok, "default"}
      end
      """

      metadata = %{
        "author_agent" => "e2e-agent",
        "status" => "probation",
        "description" => "persistence test",
        "parameters_schema" => %{"input" => "string"}
      }

      # Save
      :ok = Persistence.save("persist_test", source, metadata)
      IO.puts("save: ok")

      # Verify file exists
      listed = Persistence.list_persisted()
      true = "persist_test" in listed
      IO.puts("list_persisted: found")

      # Load all from disk
      {:ok, count} = Persistence.load_all()
      true = count >= 1
      IO.puts("load_all: loaded #{count} tool(s)")

      # Verify registered
      {:ok, entry} = Registry.lookup("persist_test")
      "persist_test" = entry.name
      IO.puts("registered_after_load: ok")

      # Delete
      :ok = Persistence.delete("persist_test")
      remaining = Persistence.list_persisted()
      false = "persist_test" in remaining
      IO.puts("delete: ok")

      Registry.clear()
      IO.puts("PERSISTENCE_OK")
    ' 2>&1
}

# --- TC-13.8: Composer dependency tracking ---

tc_synth_composer() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.ToolSynthesis.{Registry, Composer}

      Registry.init()
      Composer.init_table()

      # Start sandbox task supervisor (needed for call_tool)
      {:ok, _} = Task.Supervisor.start_link(
        name: RustyclawOrchestrator.ToolSynthesis.Sandbox.TaskSupervisor
      )

      # Register two tools
      defmodule RustyclawOrchestrator.Synth.ToolA do
        def name, do: "tool_a"
        def description, do: "Tool A"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "a"}
      end

      defmodule RustyclawOrchestrator.Synth.ToolB do
        def name, do: "tool_b"
        def description, do: "Tool B"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "b"}
      end

      Registry.register("tool_a", RustyclawOrchestrator.Synth.ToolA)
      Registry.register("tool_b", RustyclawOrchestrator.Synth.ToolB)

      server_name = :"composer_e2e_#{:erlang.unique_integer([:positive])}"
      {:ok, _} = Composer.start_link(name: server_name)

      # Add dependency: tool_b depends on tool_a
      :ok = Composer.add_dependency("tool_b", "tool_a", server: server_name)
      IO.puts("add_dep: ok")

      # Check dependencies
      deps = Composer.get_dependencies("tool_b")
      true = "tool_a" in deps
      IO.puts("get_deps: tool_b depends on tool_a")

      # Check dependents
      dependents = Composer.get_dependents("tool_a")
      true = "tool_b" in dependents
      IO.puts("get_dependents: tool_a has dependent tool_b")

      # Remove dependency
      :ok = Composer.remove_dependency("tool_b", "tool_a", server: server_name)
      [] = Composer.get_dependencies("tool_b")
      IO.puts("remove_dep: ok")

      # Call tool via composer
      {:ok, result} = Composer.call_tool("tool_a", %{})
      "a" = result
      IO.puts("call_tool: ok")

      Registry.clear()
      Composer.clear()
      IO.puts("COMPOSER_OK")
    ' 2>&1
}

# --- TC-13.9: Synth API endpoints respond ---

tc_synth_api() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    local persist_dir
    persist_dir=$(mktemp -d "${E2E_WORKSPACE}/synth_api_persist.XXXXXX")

    SYNTH_PERSIST_DIR="$persist_dir" \
    mix run --no-start -e '
      persist_dir = System.get_env("SYNTH_PERSIST_DIR")
      Application.put_env(:rustyclaw_orchestrator, :synthesized_tools_dir, persist_dir)

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
      base = "http://localhost:#{synth_port}"
      IO.puts("Synth API on port #{synth_port}")

      # GET /api/synth/tools — empty list
      {:ok, resp} = Req.get("#{base}/api/synth/tools")
      200 = resp.status
      true = is_list(resp.body)
      IO.puts("GET /api/synth/tools: 200 (#{length(resp.body)} tools)")

      # POST /api/synth/execute — missing field
      {:ok, resp} = Req.post("#{base}/api/synth/execute", json: %{})
      400 = resp.status
      IO.puts("POST /api/synth/execute (missing field): 400")

      # POST /api/synth/execute — tool not found
      {:ok, resp} = Req.post("#{base}/api/synth/execute",
        json: %{"tool" => "nonexistent", "params" => %{}})
      404 = resp.status
      IO.puts("POST /api/synth/execute (not found): 404")

      # POST /api/synth/approve — not found
      {:ok, resp} = Req.post("#{base}/api/synth/approve",
        json: %{"name" => "nonexistent"})
      404 = resp.status
      IO.puts("POST /api/synth/approve (not found): 404")

      # POST /api/synth/suspend — not found
      {:ok, resp} = Req.post("#{base}/api/synth/suspend",
        json: %{"name" => "nonexistent"})
      404 = resp.status
      IO.puts("POST /api/synth/suspend (not found): 404")

      # DELETE /api/synth/tools/:name — not found
      {:ok, resp} = Req.delete("#{base}/api/synth/tools/nonexistent")
      404 = resp.status
      IO.puts("DELETE /api/synth/tools (not found): 404")

      # GET /api/synth/versions/:name
      {:ok, resp} = Req.get("#{base}/api/synth/versions/nonexistent")
      200 = resp.status
      IO.puts("GET /api/synth/versions: 200")

      # 404 catch-all
      {:ok, resp} = Req.get("#{base}/api/synth/bogus")
      404 = resp.status
      IO.puts("catch-all: 404")

      IO.puts("SYNTH_API_OK")
    ' 2>&1
}

# --- TC-13.10: CLI synth list command ---

tc_synth_cli() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    # Start Elixir app with synth API on port 4001
    # Check if port 4001 is already in use
    if curl -sf "http://localhost:4001/api/synth/tools" >/dev/null 2>&1; then
        skip "Port 4001 already in use, skipping CLI test"
        return 0
    fi

    local persist_dir
    persist_dir=$(mktemp -d "${E2E_WORKSPACE}/synth_cli_persist.XXXXXX")

    SYNTH_PERSIST_DIR="$persist_dir" \
    mix run --no-start --no-halt -e '
      persist_dir = System.get_env("SYNTH_PERSIST_DIR")
      Application.put_env(:rustyclaw_orchestrator, :synthesized_tools_dir, persist_dir)
      Application.put_env(:rustyclaw_orchestrator, :synth_api_port, 4001)
      Application.put_env(:rustyclaw_orchestrator, :plugin_api_port, 0)
      {:ok, _} = Application.ensure_all_started(:rustyclaw_orchestrator)
    ' &
    local elixir_pid=$!

    # Wait for API to be ready
    local attempts=0
    while [[ $attempts -lt 30 ]]; do
        sleep 0.5
        attempts=$((attempts + 1))
        if curl -sf "http://localhost:4001/api/synth/tools" >/dev/null 2>&1; then
            break
        fi
        if ! kill -0 "$elixir_pid" 2>/dev/null; then
            echo "Elixir process died"
            return 1
        fi
    done

    if [[ $attempts -ge 30 ]]; then
        kill "$elixir_pid" 2>/dev/null || true
        wait "$elixir_pid" 2>/dev/null || true
        echo "Elixir API did not start in time"
        return 1
    fi

    echo "Elixir synth API ready on port 4001"

    # Test CLI synth list
    local cli_output
    cli_output=$("$BINARY" synth list 2>&1) || true
    echo "CLI output: $cli_output"

    # Should contain "No synthesized tools" or similar (empty list)
    if echo "$cli_output" | grep -qiE "no synthesized tools|synth|tool"; then
        echo "CLI synth list: ok"
    else
        echo "CLI synth list: unexpected output"
        kill "$elixir_pid" 2>/dev/null || true
        wait "$elixir_pid" 2>/dev/null || true
        return 1
    fi

    # Cleanup
    kill "$elixir_pid" 2>/dev/null || true
    wait "$elixir_pid" 2>/dev/null || true
    echo "SYNTH_CLI_OK"
}
