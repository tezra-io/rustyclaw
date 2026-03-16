defmodule RustyclawOrchestrator.Plugins.PluginRouterTest do
  use ExUnit.Case, async: true

  import Plug.Conn
  import Plug.Test

  alias RustyclawOrchestrator.Plugins.PluginRouter

  @opts PluginRouter.init([])

  describe "GET /api/plugins" do
    test "returns list of plugins" do
      conn =
        conn(:get, "/api/plugins")
        |> PluginRouter.call(@opts)

      assert conn.status == 200
      body = Jason.decode!(conn.resp_body)
      assert body["ok"] == true
      assert is_list(body["plugins"])
    end
  end

  describe "POST /api/plugins/exec" do
    test "returns 400 when missing capability" do
      conn =
        conn(:post, "/api/plugins/exec", %{"description" => "test"})
        |> put_req_header("content-type", "application/json")
        |> PluginRouter.call(@opts)

      assert conn.status == 400
      body = Jason.decode!(conn.resp_body)
      assert body["error"] =~ "missing field: capability"
    end

    test "returns 400 when missing description" do
      conn =
        conn(:post, "/api/plugins/exec", %{"capability" => "coding"})
        |> put_req_header("content-type", "application/json")
        |> PluginRouter.call(@opts)

      assert conn.status == 400
      body = Jason.decode!(conn.resp_body)
      assert body["error"] =~ "missing field: description"
    end

    test "returns 404 when no plugin available for capability" do
      conn =
        conn(:post, "/api/plugins/exec", %{
          "capability" => "coding",
          "description" => "Fix bug"
        })
        |> put_req_header("content-type", "application/json")
        |> PluginRouter.call(@opts)

      assert conn.status == 404
      body = Jason.decode!(conn.resp_body)
      assert body["ok"] == false
      assert body["error"] =~ "No available plugin"
    end
  end

  describe "POST /api/plugins/:name/retry/:task_id" do
    test "schedules a retry" do
      conn =
        conn(:post, "/api/plugins/claude_code/retry/task-123", %{})
        |> put_req_header("content-type", "application/json")
        |> PluginRouter.call(@opts)

      assert conn.status == 200
      body = Jason.decode!(conn.resp_body)
      assert body["ok"] == true
      assert body["message"] =~ "task-123"
    end
  end

  describe "GET /api/plugins/status" do
    test "returns status with plugins and retry count" do
      conn =
        conn(:get, "/api/plugins/status")
        |> PluginRouter.call(@opts)

      assert conn.status == 200
      body = Jason.decode!(conn.resp_body)
      assert body["ok"] == true
      assert is_list(body["plugins"])
      assert is_integer(body["pending_retries"])
    end
  end

  describe "catch-all" do
    test "returns 404 for unknown routes" do
      conn =
        conn(:get, "/api/unknown")
        |> PluginRouter.call(@opts)

      assert conn.status == 404
    end
  end
end
