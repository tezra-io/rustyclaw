defmodule RustyclawOrchestrator.SkillsInvokeTest do
  use ExUnit.Case, async: false

  import Plug.Conn
  import Plug.Test

  alias RustyclawOrchestrator.AgentSupervisor
  alias RustyclawOrchestrator.TestSupport.BridgeMock
  alias RustyclawOrchestrator.ToolSynthesis.ApiRouter

  @skills_dir Path.join(
                System.tmp_dir!(),
                "rustyclaw_skills_invoke_test_#{System.unique_integer([:positive])}"
              )

  setup do
    File.rm_rf!(@skills_dir)
    File.mkdir_p!(@skills_dir)
    Application.put_env(:rustyclaw_orchestrator, :skills_dir, @skills_dir)

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end

      File.rm_rf!(@skills_dir)
      Application.delete_env(:rustyclaw_orchestrator, :skills_dir)
    end)

    :ok
  end

  defp create_skill(name) do
    skill_dir = Path.join(@skills_dir, name)
    File.mkdir_p!(skill_dir)

    content = """
    ---
    name: #{name}
    persistent: false
    capabilities:
      - code
    ---

    You are a #{name} agent.
    """

    File.write!(Path.join(skill_dir, "SKILL.md"), content)
  end

  defp call_router(conn) do
    ApiRouter.call(conn, ApiRouter.init([]))
  end

  defp json_body(conn) do
    Jason.decode!(conn.resp_body)
  end

  defp post_invoke(body) do
    conn(:post, "/api/skills/invoke", Jason.encode!(body))
    |> put_req_header("content-type", "application/json")
    |> call_router()
  end

  describe "POST /api/skills/invoke" do
    test "returns 400 when skill name is missing" do
      conn = post_invoke(%{task: "do something"})

      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "missing field"
    end

    test "returns 400 when task is missing" do
      conn = post_invoke(%{skill: "some-skill"})

      assert conn.status == 400
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "missing field"
    end

    test "returns 404 when skill does not exist" do
      conn = post_invoke(%{skill: "nonexistent-skill", task: "do something"})

      assert conn.status == 404
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] =~ "not found"
    end

    test "spawns agent and attempts task for valid skill" do
      create_skill("test-coding-skill")

      conn = post_invoke(%{skill: "test-coding-skill", task: "hello", timeout_ms: 5_000})

      # Bridge is unreachable in tests, so we get a 500 with error
      assert conn.status == 500
      body = json_body(conn)
      assert body["ok"] == false
      assert body["error"] != nil
    end

    test "cleans up ephemeral agent after skill completes" do
      create_skill("cleanup-skill")

      _conn = post_invoke(%{skill: "cleanup-skill", task: "do work", timeout_ms: 5_000})

      # Ephemeral agent should be stopped — name has random suffix, check none remain
      agents = AgentSupervisor.list_agents()
      matching = Enum.filter(agents, &String.starts_with?(&1, "cleanup-skill"))
      assert matching == []
    end

    test "response has consistent JSON structure" do
      create_skill("structure-skill")

      conn = post_invoke(%{skill: "structure-skill", task: "check structure", timeout_ms: 5_000})

      body = json_body(conn)
      assert is_boolean(body["ok"])
    end

    test "returns success when bridge is available" do
      BridgeMock.setup()
      create_skill("bridge-skill")

      conn = post_invoke(%{skill: "bridge-skill", task: "do work", timeout_ms: 10_000})

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert is_map(body["result"])
      assert body["result"]["status"] == "completed"
    end

    test "forwards context to the task" do
      %{bypass: bypass} = BridgeMock.setup()

      # Replace the stub to capture the task body
      test_pid = self()

      Bypass.expect(bypass, "POST", "/api/agent/run", fn conn ->
        {:ok, req_body, conn} = Plug.Conn.read_body(conn)
        request = Jason.decode!(req_body)
        send(test_pid, {:captured_task, request["task"]})

        conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(200, Jason.encode!(%{"status" => "completed"}))
      end)

      create_skill("ctx-skill")

      _conn =
        post_invoke(%{
          skill: "ctx-skill",
          task: "deploy staging",
          context: "Branch: main, Commit: abc123",
          timeout_ms: 10_000
        })

      assert_receive {:captured_task, task}, 5_000
      assert task =~ "Branch: main, Commit: abc123"
      assert task =~ "deploy staging"
    end

    test "cleans up ephemeral agent after successful bridge invocation" do
      BridgeMock.setup()
      create_skill("success-cleanup-skill")

      conn =
        post_invoke(%{
          skill: "success-cleanup-skill",
          task: "do work",
          timeout_ms: 10_000
        })

      assert conn.status == 200

      # Agent should be stopped — name has random suffix, check none remain with the prefix
      agents = AgentSupervisor.list_agents()
      matching = Enum.filter(agents, &String.starts_with?(&1, "success-cleanup-skill"))
      assert matching == []
    end
  end

  describe "GET /api/skills" do
    test "returns empty list when no skills exist" do
      conn =
        conn(:get, "/api/skills")
        |> call_router()

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert body["skills"] == []
    end

    test "lists available skills" do
      create_skill("list-skill-a")
      create_skill("list-skill-b")

      conn =
        conn(:get, "/api/skills")
        |> call_router()

      assert conn.status == 200
      body = json_body(conn)
      assert body["ok"] == true
      assert "list-skill-a" in body["skills"]
      assert "list-skill-b" in body["skills"]
    end
  end
end
