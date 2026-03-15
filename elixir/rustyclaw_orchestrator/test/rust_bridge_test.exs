defmodule RustyclawOrchestrator.RustBridgeTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.RustBridge

  # RustBridge is started by the application supervisor.

  describe "base_url/0" do
    test "returns configured base URL" do
      url = RustBridge.base_url()
      assert is_binary(url)
      assert String.starts_with?(url, "http")
    end
  end

  describe "health_check/0 (no Rust core running)" do
    test "returns error when Rust core is not reachable" do
      assert {:error, _reason} = RustBridge.health_check()
    end
  end

  describe "run_task/3 (no Rust core running)" do
    @tag timeout: 30_000
    test "returns max_retries_exceeded when Rust core is unreachable" do
      assert {:error, :max_retries_exceeded} = RustBridge.run_task("test-agent", "do something")
    end
  end

  describe "concurrent calls (non-blocking GenServer)" do
    setup do
      bypass = Bypass.open()

      {:ok, bridge} =
        RustBridge.start_link(
          name: :"test_bridge_#{System.unique_integer([:positive])}",
          base_url: "http://localhost:#{bypass.port}",
          max_retries: 1,
          connect_timeout: 5_000
        )

      %{bypass: bypass, bridge: bridge}
    end

    test "base_url responds while HTTP call is in-flight", %{bypass: bypass, bridge: bridge} do
      Bypass.stub(bypass, "POST", "/api/agent/run", fn conn ->
        Process.sleep(1_000)
        Plug.Conn.resp(conn, 200, Jason.encode!(%{status: "ok"}))
      end)

      # Fire a slow run_task call asynchronously
      caller =
        Task.async(fn ->
          GenServer.call(bridge, {:run_task, "test", "slow task", []}, 5_000)
        end)

      # Give the task a moment to start the HTTP request
      Process.sleep(50)

      # base_url should respond immediately — proves GenServer is not blocked
      assert GenServer.call(bridge, :base_url, 1_000) =~ "http"

      Task.await(caller, 5_000)
    end

    test "multiple concurrent calls complete without serializing", %{
      bypass: bypass,
      bridge: bridge
    } do
      Bypass.stub(bypass, "POST", "/api/agent/run", fn conn ->
        Process.sleep(500)
        Plug.Conn.resp(conn, 200, Jason.encode!(%{result: "done"}))
      end)

      # Fire 3 concurrent calls
      tasks =
        for i <- 1..3 do
          Task.async(fn ->
            GenServer.call(bridge, {:run_task, "agent-#{i}", "task", []}, 5_000)
          end)
        end

      start = System.monotonic_time(:millisecond)
      results = Task.await_many(tasks, 5_000)
      elapsed = System.monotonic_time(:millisecond) - start

      assert Enum.all?(results, &match?({:ok, _}, &1))
      # Parallel: ~500ms. Serial would be ~1500ms.
      assert elapsed < 1_200
    end
  end
end
