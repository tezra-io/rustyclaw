defmodule RustyclawOrchestrator.AgentApiTest do
  use ExUnit.Case, async: false

  import Plug.Conn
  import Plug.Test

  alias RustyclawOrchestrator.{AgentDefinition, AgentSupervisor}
  alias RustyclawOrchestrator.ToolSynthesis.ApiRouter

  @bridge_secret "test-bridge-secret-agent-api"

  setup do
    System.put_env("RUSTYCLAW_BRIDGE_SECRET", @bridge_secret)

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      System.delete_env("RUSTYCLAW_BRIDGE_SECRET")
    end)

    :ok
  end

  # --- Helpers ---

  defp call_router(conn) do
    ApiRouter.call(conn, ApiRouter.init([]))
  end

  defp json_body(conn) do
    Jason.decode!(conn.resp_body)
  end

  defp authed_post(path, body) do
    conn(:post, path, Jason.encode!(body))
    |> put_req_header("content-type", "application/json")
    |> put_req_header("x-bridge-secret", @bridge_secret)
    |> call_router()
  end

  defp authed_get(path) do
    conn(:get, path)
    |> put_req_header("x-bridge-secret", @bridge_secret)
    |> call_router()
  end

  defp authed_delete(path) do
    conn(:delete, path)
    |> put_req_header("x-bridge-secret", @bridge_secret)
    |> call_router()
  end

  defp spawn_test_agent(name, opts \\ []) do
    definition = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, ["test"]),
      personality: Keyword.get(opts, :personality, "Test agent")
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(definition)
  end

  # --- POST /api/agents/spawn ---

  describe "POST /api/agents/spawn" do
    test "rejects unauthenticated requests" do
      conn =
        conn(:post, "/api/agents/spawn", Jason.encode!(%{name: "x"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status in [401, 403]
    end

    test "spawns an agent successfully" do
      conn = authed_post("/api/agents/spawn", %{name: "api-spawn-test"})

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["agent_name"] == "api-spawn-test"
      assert is_binary(body["pid"])
    end

    test "returns 422 for missing name" do
      conn = authed_post("/api/agents/spawn", %{capabilities: ["test"]})

      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
      assert is_binary(body["error"])
    end

    test "returns 422 for duplicate name" do
      spawn_test_agent("api-dup-agent")
      conn = authed_post("/api/agents/spawn", %{name: "api-dup-agent"})

      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
    end
  end

  # --- GET /api/agents ---

  describe "GET /api/agents" do
    test "rejects unauthenticated requests" do
      conn =
        conn(:get, "/api/agents")
        |> call_router()

      assert conn.status in [401, 403]
    end

    test "returns empty list when no agents running" do
      conn = authed_get("/api/agents")

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["agents"] == []
      assert body["count"] == 0
    end

    test "lists running agents" do
      spawn_test_agent("api-list-a")
      spawn_test_agent("api-list-b")

      conn = authed_get("/api/agents")

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["count"] >= 2

      names = Enum.map(body["agents"], & &1["name"])
      assert "api-list-a" in names
      assert "api-list-b" in names
    end

    test "filters by capability" do
      spawn_test_agent("api-cap-a", capabilities: ["search"])
      spawn_test_agent("api-cap-b", capabilities: ["code"])

      conn =
        conn(:get, "/api/agents?capability=search")
        |> put_req_header("x-bridge-secret", @bridge_secret)
        |> call_router()

      assert conn.status == 200
      body = json_body(conn)
      names = Enum.map(body["agents"], & &1["name"])
      assert "api-cap-a" in names
      refute "api-cap-b" in names
    end
  end

  # --- DELETE /api/agents/:name ---

  describe "DELETE /api/agents/:name" do
    test "rejects unauthenticated requests" do
      conn =
        conn(:delete, "/api/agents/some-agent")
        |> call_router()

      assert conn.status in [401, 403]
    end

    test "kills a running agent" do
      spawn_test_agent("api-kill-target")

      conn = authed_delete("/api/agents/api-kill-target")

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["killed"] == true
      assert body["agent_name"] == "api-kill-target"
    end

    test "returns 422 for nonexistent agent" do
      conn = authed_delete("/api/agents/ghost-agent")

      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
      assert is_binary(body["error"])
    end
  end

  # --- POST /api/agents/message ---

  describe "POST /api/agents/message" do
    test "rejects unauthenticated requests" do
      conn =
        conn(:post, "/api/agents/message", Jason.encode!(%{target: "x", message: "hi"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status in [401, 403]
    end

    test "returns 422 for nonexistent target" do
      conn = authed_post("/api/agents/message", %{target: "ghost", message: "hello"})

      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
      assert is_binary(body["error"])
    end

    test "delivers async message to running agent" do
      spawn_test_agent("api-msg-target")

      conn =
        authed_post("/api/agents/message", %{
          target: "api-msg-target",
          message: "hello there"
        })

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["delivered"] == true
    end
  end

  # --- POST /api/agents/delegate ---

  describe "POST /api/agents/delegate" do
    test "rejects unauthenticated requests" do
      conn =
        conn(:post, "/api/agents/delegate", Jason.encode!(%{task: "do something"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status in [401, 403]
    end

    test "returns 422 when no matching agents" do
      conn =
        authed_post("/api/agents/delegate", %{
          task: "do impossible thing",
          capabilities: ["nonexistent_capability"]
        })

      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
      assert is_binary(body["error"])
    end

    test "rejects non-list capabilities" do
      conn =
        authed_post("/api/agents/delegate", %{task: "do something", capabilities: "not-a-list"})

      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "capabilities must be a list"
    end

    test "rejects non-string capability entries" do
      conn = authed_post("/api/agents/delegate", %{task: "do something", capabilities: [1, 2]})
      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "capabilities must be a list of strings"
    end

    test "rejects unknown strategy" do
      conn = authed_post("/api/agents/delegate", %{task: "do something", strategy: "yolo"})
      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "unknown strategy"
    end

    test "accepts timeout_ms parameter" do
      conn =
        authed_post("/api/agents/delegate", %{
          task: "do something",
          capabilities: ["nonexistent"],
          timeout_ms: 5000
        })

      # Should process normally (will fail with no_matching_agents, not timeout error)
      assert conn.status == 422
      body = json_body(conn)
      assert body["ok"] == false
    end
  end
end
