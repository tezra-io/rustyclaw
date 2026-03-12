import Config

# RustBridge: no retries and short connect timeout in tests since there's no Rust core running
config :rustyclaw_orchestrator, :rust_bridge, max_retries: 0, connect_timeout: 100
