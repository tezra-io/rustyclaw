import Config

# UDS bridge: preferred transport when the Rust daemon passes a socket path.
# Falls back to HTTP-over-TCP if RUSTYCLAW_BRIDGE_SOCKET is not set.
if bridge_socket = System.get_env("RUSTYCLAW_BRIDGE_SOCKET") do
  config :rustyclaw_orchestrator, :rust_bridge,
    base_url: "http://localhost",
    unix_socket: bridge_socket
else
  if bridge_port = System.get_env("RUSTYCLAW_BRIDGE_PORT") do
    config :rustyclaw_orchestrator, :rust_bridge, base_url: "http://localhost:#{bridge_port}"
  end
end

# Elixir API ports — Rust daemon passes these so both sides agree on ports.
if synth_port = System.get_env("RUSTYCLAW_ELIXIR_SYNTH_PORT") do
  config :rustyclaw_orchestrator, :synth_api_port, String.to_integer(synth_port)
end

if plugin_port = System.get_env("RUSTYCLAW_ELIXIR_PLUGIN_PORT") do
  config :rustyclaw_orchestrator, :plugin_api_port, String.to_integer(plugin_port)
end

# Bridge secret for authenticating Rust → Elixir API calls
if bridge_secret = System.get_env("RUSTYCLAW_BRIDGE_SECRET") do
  config :rustyclaw_orchestrator, :bridge_secret, bridge_secret
end
