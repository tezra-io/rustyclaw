#!/usr/bin/env bash
# Suite: Elixir Orchestration — AgentServer, Coordinator, RustBridge, OTP

suite_06-elixir() {
    # Run Elixir unit tests as part of E2E (they already cover integration)
    run_test "TC-6.1" "Elixir compile (warnings-as-errors)" tc_elixir_compile
    run_test "TC-6.2" "Elixir credo --strict" tc_elixir_credo
    run_test "TC-6.3" "Elixir test suite" tc_elixir_test 120
}

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
