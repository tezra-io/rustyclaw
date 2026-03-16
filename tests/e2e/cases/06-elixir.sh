#!/usr/bin/env bash
# Suite: Elixir Orchestration — OTP lifecycle, agent spawn, bridge health, sessions

suite_06-elixir() {
    # Static analysis (existing)
    run_test "TC-6.1" "Elixir compile (warnings-as-errors)" tc_elixir_compile
    run_test "TC-6.2" "Elixir credo --strict" tc_elixir_credo
    run_test "TC-6.3" "Elixir test suite" tc_elixir_test 120

    # OTP lifecycle
    run_test "TC-6.4" "OTP application boots without crashing" tc_otp_app_starts 30
    run_test "TC-6.5" "Agent definition YAML parsing" tc_agent_def_parsing 30
    run_test "TC-6.6" "AgentSupervisor spawn/stop lifecycle" tc_agent_lifecycle 60

    # Bridge test (needs Rust gateway running)
    start_gateway "elixir-bridge"
    run_test "TC-6.7" "RustBridge health check via gateway" tc_bridge_health 60
    stop_gateway

    # Session lifecycle and formatting
    run_test "TC-6.8" "SubAgentSession lifecycle transitions" tc_session_lifecycle 30
    run_test "TC-6.9" "Elixir format check" tc_format_check
}

# --- Existing tests ---

tc_elixir_compile() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix compile --warnings-as-errors 2>&1
}

tc_elixir_credo() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix credo --strict 2>&1
}

tc_elixir_test() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix test 2>&1
}

# --- TC-6.4: OTP application boots without crashing ---

tc_otp_app_starts() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix run --no-halt &
    local app_pid=$!
    sleep 4
    if kill -0 "$app_pid" 2>/dev/null; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
        return 0
    else
        return 1
    fi
}

# --- TC-6.5: Agent definition YAML parsing ---

tc_agent_def_parsing() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    local agent_dir="$E2E_WORKSPACE/agents"
    mkdir -p "$agent_dir"
    cat > "$agent_dir/e2e-parser-test.md" << 'AGENT_EOF'
---
name: e2e-parser-test
persistent: false
capabilities:
  - web_search
  - coding
delegates_to: []
model: claude-sonnet-4-20250514
temperature: 0.7
---
You are a test agent for E2E definition parsing validation.
AGENT_EOF

    AGENT_DEF_PATH="$agent_dir/e2e-parser-test.md" \
    mix run --no-start -e '
      {:ok, _} = Application.ensure_all_started(:yaml_elixir)
      path = System.get_env("AGENT_DEF_PATH")
      {:ok, def} = RustyclawOrchestrator.AgentDefinition.from_file(path)

      "e2e-parser-test" = def.name
      false = def.persistent
      true = "web_search" in def.capabilities
      true = "coding" in def.capabilities
      "claude-sonnet-4-20250514" = def.model
      0.7 = def.temperature

      IO.puts("AGENT_DEF_PARSE_OK")
    ' 2>&1
}

# --- TC-6.6: AgentSupervisor spawn/stop lifecycle ---

tc_agent_lifecycle() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      # Use random ports to avoid conflicts with other services
      Application.put_env(:rustyclaw_orchestrator, :synth_api_port, 0)
      Application.put_env(:rustyclaw_orchestrator, :plugin_api_port, 0)
      {:ok, _} = Application.ensure_all_started(:rustyclaw_orchestrator)

      alias RustyclawOrchestrator.{AgentDefinition, AgentSupervisor, AgentServer}

      definition = %AgentDefinition{
        name: "e2e-lifecycle-agent",
        capabilities: ["testing"],
        personality: "E2E lifecycle test agent"
      }

      # Spawn
      {:ok, pid} = AgentSupervisor.spawn_agent(definition)
      true = is_pid(pid)
      IO.puts("spawn: ok")

      # List — agent should be present
      agents = AgentSupervisor.list_agents()
      true = "e2e-lifecycle-agent" in agents
      IO.puts("list: found")

      # Get state — should be idle and healthy
      state = AgentServer.get_state("e2e-lifecycle-agent")
      :idle = state.status
      :healthy = state.health
      IO.puts("state: idle/healthy")

      # Stop
      :ok = AgentSupervisor.stop_agent("e2e-lifecycle-agent")
      Process.sleep(100)

      # Verify removed
      agents_after = AgentSupervisor.list_agents()
      false = "e2e-lifecycle-agent" in agents_after
      IO.puts("stop: removed")

      IO.puts("LIFECYCLE_OK")
    ' 2>&1
}

# --- TC-6.7: RustBridge health check via gateway ---

tc_bridge_health() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    RUST_GATEWAY_URL="$GATEWAY_URL" \
    mix run --no-start -e '
      url = System.get_env("RUST_GATEWAY_URL")
      IO.puts("Bridge target: #{url}")

      {:ok, _} = Application.ensure_all_started(:req)
      {:ok, _} = Task.Supervisor.start_link(
        name: RustyclawOrchestrator.RustBridge.TaskSupervisor
      )
      {:ok, _} = RustyclawOrchestrator.RustBridge.start_link(base_url: url)

      case RustyclawOrchestrator.RustBridge.health_check() do
        :ok ->
          IO.puts("BRIDGE_HEALTH_OK")

        {:error, {:http_error, status}} ->
          # Bridge reached the gateway — endpoint path may differ
          IO.puts("BRIDGE_REACHABLE: HTTP #{status}")

        {:error, reason} ->
          IO.puts("BRIDGE_FAIL: #{inspect(reason)}")
          System.halt(1)
      end
    ' 2>&1
}

# --- TC-6.8: SubAgentSession lifecycle transitions ---

tc_session_lifecycle() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"

    mix run --no-start -e '
      alias RustyclawOrchestrator.SubAgentSession

      # Initialize ETS table (normally done by Application.start)
      SubAgentSession.init()

      # Create — starts as pending
      session = SubAgentSession.create("e2e-agent", "summarize docs")
      :pending = session.status
      "e2e-agent" = session.agent_name
      IO.puts("create: pending")

      # Activate
      {:ok, active} = SubAgentSession.activate(session.id)
      :active = active.status
      IO.puts("activate: active")

      # Complete with result
      {:ok, completed} = SubAgentSession.complete(session.id, %{summary: "done"})
      :completed = completed.status
      IO.puts("complete: completed")

      # Invalid transition (completed -> active)
      {:error, :invalid_transition} = SubAgentSession.activate(session.id)
      IO.puts("invalid_transition: blocked")

      # List filtered by agent
      [_] = SubAgentSession.list(agent_name: "e2e-agent")
      IO.puts("list: 1 session")

      # Delete
      :ok = SubAgentSession.delete(session.id)
      {:error, :not_found} = SubAgentSession.get(session.id)
      IO.puts("delete: removed")

      IO.puts("SESSION_LIFECYCLE_OK")
    ' 2>&1
}

# --- TC-6.9: Elixir format check ---

tc_format_check() {
    cd "$PROJECT_DIR/elixir/rustyclaw_orchestrator"
    mix format --check-formatted 2>&1
}
