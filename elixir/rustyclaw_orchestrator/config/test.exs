import Config

# RustBridge: no retries and short connect timeout in tests since there's no Rust core running
config :rustyclaw_orchestrator, :rust_bridge, max_retries: 0, connect_timeout: 100

# Use temp dir for snapshot isolation in tests
config :rustyclaw_orchestrator,
  snapshot_dir: Path.join(System.tmp_dir!(), "rustyclaw_test_snapshots")

# Use temp dir for agent definitions in tests (avoids picking up real agent files)
config :rustyclaw_orchestrator,
  agents_dir: Path.join(System.tmp_dir!(), "rustyclaw_test_agents")

# Use temp dir for synthesized tools in tests
config :rustyclaw_orchestrator,
  synthesized_tools_dir: Path.join(System.tmp_dir!(), "rustyclaw_test_synth_tools")
