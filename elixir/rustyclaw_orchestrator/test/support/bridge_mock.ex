defmodule RustyclawOrchestrator.TestSupport.BridgeMock do
  @moduledoc false

  @doc """
  Reconfigure the global RustBridge to route HTTP calls through a Bypass server.

  Call from a `setup` block. Registers an `on_exit` callback that restores the
  original bridge state after each test.

  The mock stubs POST /api/agent/run to echo the task back as a JSON response.
  """
  def setup do
    bypass = Bypass.open()
    original_state = :sys.get_state(RustyclawOrchestrator.RustBridge)

    bypass_url = "http://localhost:#{bypass.port}"

    :sys.replace_state(RustyclawOrchestrator.RustBridge, fn state ->
      %{
        state
        | base_url: bypass_url,
          max_retries: 1,
          req:
            Req.new(
              base_url: bypass_url,
              headers: [{"content-type", "application/json"}],
              receive_timeout: 5_000,
              connect_options: [timeout: 1_000],
              retry: false
            )
      }
    end)

    Bypass.stub(bypass, "POST", "/api/agent/run", fn conn ->
      {:ok, body, conn} = Plug.Conn.read_body(conn)
      request = Jason.decode!(body)

      response = %{
        "task" => request["task"],
        "agent" => request["agent"],
        "status" => "completed"
      }

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.resp(200, Jason.encode!(response))
    end)

    Bypass.stub(bypass, "GET", "/api/health", fn conn ->
      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.resp(200, Jason.encode!(%{"status" => "ok"}))
    end)

    ExUnit.Callbacks.on_exit(fn ->
      :sys.replace_state(RustyclawOrchestrator.RustBridge, fn _state ->
        original_state
      end)
    end)

    %{bypass: bypass}
  end
end
