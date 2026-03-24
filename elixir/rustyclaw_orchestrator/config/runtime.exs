import Config

# Read the gateway port from the environment variable set by the Rust daemon.
# This is the port where the Rust core's HTTP API (gateway) is listening.
if bridge_port = System.get_env("RUSTYCLAW_BRIDGE_PORT") do
  config :rustyclaw_orchestrator, :rust_bridge, base_url: "http://localhost:#{bridge_port}"
end

# Bridge secret for authenticating Rust → Elixir API calls
if bridge_secret = System.get_env("RUSTYCLAW_BRIDGE_SECRET") do
  config :rustyclaw_orchestrator, :bridge_secret, bridge_secret
end
