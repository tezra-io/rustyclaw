defmodule RustyclawOrchestrator.ToolSynthesis.ApiRouterTest do
  use ExUnit.Case, async: false

  import Plug.Conn
  import Plug.Test

  alias RustyclawOrchestrator.ToolSynthesis.{ApiRouter, Registry}

  # --- Test tool modules ---

  defmodule UppercaseTool do
    def name, do: "uppercase"
    def description, do: "Uppercases input text"

    def parameters_schema,
      do: %{"type" => "object", "properties" => %{"text" => %{"type" => "string"}}}

    def execute(%{"text" => text}), do: {:ok, String.upcase(text)}
    def execute(_), do: {:error, "missing text param"}
  end

  defmodule FailTool do
    def name, do: "fail_tool"
    def description, do: "Always fails"
    def parameters_schema, do: %{"type" => "object"}
    def execute(_), do: {:error, "intentional failure"}
  end

  setup do
    Registry.clear()
    :ok
  end

  # --- Helper ---

  defp call_router(conn) do
    ApiRouter.call(conn, ApiRouter.init([]))
  end

  defp json_body(conn) do
    Jason.decode!(conn.resp_body)
  end

  # --- GET /api/synth/tools ---

  describe "GET /api/synth/tools" do
    test "returns empty list when no tools registered" do
      conn =
        :get
        |> conn("/api/synth/tools")
        |> call_router()

      assert conn.status == 200
      assert json_body(conn) == []
    end

    test "returns registered tools" do
      Registry.register("uppercase", UppercaseTool, author_agent: "agent1", status: :promoted)

      conn =
        :get
        |> conn("/api/synth/tools")
        |> call_router()

      assert conn.status == 200
      [tool] = json_body(conn)
      assert tool["name"] == "uppercase"
      assert tool["description"] == "Uppercases input text"
      assert tool["status"] == "promoted"
      assert tool["invocation_count"] == 0
    end

    test "returns multiple tools" do
      Registry.register("uppercase", UppercaseTool, status: :promoted)
      Registry.register("fail_tool", FailTool, status: :probation)

      conn =
        :get
        |> conn("/api/synth/tools")
        |> call_router()

      assert conn.status == 200
      tools = json_body(conn)
      assert length(tools) == 2
      names = Enum.map(tools, & &1["name"])
      assert "uppercase" in names
      assert "fail_tool" in names
    end
  end

  # --- POST /api/synth/execute ---

  describe "POST /api/synth/execute" do
    test "executes a registered tool successfully" do
      Registry.register("uppercase", UppercaseTool, status: :promoted)

      conn =
        :post
        |> conn(
          "/api/synth/execute",
          Jason.encode!(%{tool: "uppercase", params: %{text: "hello"}})
        )
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["output"] == "HELLO"
    end

    test "returns error for failed tool execution" do
      Registry.register("fail_tool", FailTool, status: :probation)

      conn =
        :post
        |> conn("/api/synth/execute", Jason.encode!(%{tool: "fail_tool", params: %{}}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] == "intentional failure"
    end

    test "returns 404 for unknown tool" do
      conn =
        :post
        |> conn("/api/synth/execute", Jason.encode!(%{tool: "nonexistent", params: %{}}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 404
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "not found"
    end

    test "returns 400 for missing fields" do
      conn =
        :post
        |> conn("/api/synth/execute", Jason.encode!(%{tool: "x"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 400
      body = json_body(conn)
      assert body["error"] =~ "missing field"
    end

    test "rejects execution of suspended tool" do
      Registry.register("uppercase", UppercaseTool, status: :suspended)

      conn =
        :post
        |> conn("/api/synth/execute", Jason.encode!(%{tool: "uppercase", params: %{text: "hi"}}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "suspended"
    end

    test "updates metrics after execution" do
      Registry.register("uppercase", UppercaseTool, status: :promoted)

      :post
      |> conn("/api/synth/execute", Jason.encode!(%{tool: "uppercase", params: %{text: "hi"}}))
      |> put_req_header("content-type", "application/json")
      |> call_router()

      {:ok, entry} = Registry.lookup("uppercase")
      assert entry.invocation_count == 1
      assert entry.success_count == 1
    end
  end

  # --- POST /api/synth/approve ---

  describe "POST /api/synth/approve" do
    test "promotes a tool" do
      Registry.register("uppercase", UppercaseTool, status: :probation)

      conn =
        :post
        |> conn("/api/synth/approve", Jason.encode!(%{name: "uppercase"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 200
      assert json_body(conn)["ok"] == true

      {:ok, entry} = Registry.lookup("uppercase")
      assert entry.status == :promoted
    end

    test "returns 404 for unknown tool" do
      conn =
        :post
        |> conn("/api/synth/approve", Jason.encode!(%{name: "nonexistent"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 404
    end

    test "returns 400 for missing name" do
      conn =
        :post
        |> conn("/api/synth/approve", Jason.encode!(%{}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 400
    end
  end

  # --- POST /api/synth/suspend ---

  describe "POST /api/synth/suspend" do
    test "suspends a tool" do
      Registry.register("uppercase", UppercaseTool, status: :promoted)

      conn =
        :post
        |> conn("/api/synth/suspend", Jason.encode!(%{name: "uppercase"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 200
      assert json_body(conn)["ok"] == true

      {:ok, entry} = Registry.lookup("uppercase")
      assert entry.status == :suspended
    end

    test "returns 404 for unknown tool" do
      conn =
        :post
        |> conn("/api/synth/suspend", Jason.encode!(%{name: "nonexistent"}))
        |> put_req_header("content-type", "application/json")
        |> call_router()

      assert conn.status == 404
    end
  end

  # --- DELETE /api/synth/tools/:name ---

  describe "DELETE /api/synth/tools/:name" do
    test "deletes a registered tool" do
      Registry.register("uppercase", UppercaseTool, status: :promoted)

      conn =
        :delete
        |> conn("/api/synth/tools/uppercase")
        |> call_router()

      assert conn.status == 200
      assert json_body(conn)["ok"] == true
      assert {:error, :not_found} == Registry.lookup("uppercase")
    end

    test "returns 404 for unknown tool" do
      conn =
        :delete
        |> conn("/api/synth/tools/nonexistent")
        |> call_router()

      assert conn.status == 404
    end
  end

  # --- Catch-all ---

  describe "unknown routes" do
    test "returns 404 for unknown path" do
      conn =
        :get
        |> conn("/api/unknown")
        |> call_router()

      assert conn.status == 404
    end
  end
end
