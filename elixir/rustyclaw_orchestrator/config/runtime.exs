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

# Bridge secret for authenticating Rust → Elixir API calls
if bridge_secret = System.get_env("RUSTYCLAW_BRIDGE_SECRET") do
  config :rustyclaw_orchestrator, :bridge_secret, bridge_secret
end
