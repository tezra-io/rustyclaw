defmodule RustyclawOrchestrator.Plugins.ClaudeCodePluginTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.ClaudeCodePlugin

  describe "connect/1" do
    test "succeeds with valid API key" do
      assert {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test-123"})
      assert state.api_key == "sk-test-123"
      assert state.model == "claude-sonnet-4-5-20250514"
      assert state.max_tokens == 16_384
      assert state.messages == []
    end

    test "accepts custom model and max_tokens" do
      config = %{api_key: "sk-test", model: "claude-opus-4-20250514", max_tokens: 8192}
      assert {:ok, state} = ClaudeCodePlugin.connect(config)
      assert state.model == "claude-opus-4-20250514"
      assert state.max_tokens == 8192
    end

    test "fails with missing API key" do
      assert {:error, :missing_api_key} = ClaudeCodePlugin.connect(%{})
    end

    test "fails with empty API key" do
      assert {:error, :missing_api_key} = ClaudeCodePlugin.connect(%{api_key: ""})
    end
  end

  describe "execute/3 with Bypass" do
    setup do
      bypass = Bypass.open()
      {:ok, bypass: bypass, api_base: "http://localhost:#{bypass.port}/v1/messages"}
    end

    test "handles complete response with text content", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{
          "id" => "msg_123",
          "type" => "message",
          "role" => "assistant",
          "content" => [%{"type" => "text", "text" => "Hello, world!"}],
          "stop_reason" => "end_turn"
        })

      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("content-type", "application/json")
        |> Plug.Conn.put_resp_header("x-ratelimit-remaining-requests", "42")
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      events = collect_events()
      task = %{description: "Say hello"}

      assert {:ok, {:complete, result}, new_state} =
               ClaudeCodePlugin.execute(state, task, events.handler)

      assert result.output == "Hello, world!"
      assert new_state.last_rate_limit.remaining == 42
      assert new_state.last_rate_limit.limited == false

      received_events = events.collect.()
      assert Enum.any?(received_events, &match?({:chunk, _}, &1))
    end

    test "handles tool_use response", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{
          "id" => "msg_456",
          "type" => "message",
          "role" => "assistant",
          "content" => [
            %{"type" => "text", "text" => "I'll run the tests."},
            %{
              "type" => "tool_use",
              "id" => "toolu_01",
              "name" => "shell",
              "input" => %{"cmd" => "cargo test"}
            }
          ],
          "stop_reason" => "tool_use"
        })

      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("content-type", "application/json")
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      events = collect_events()
      task = %{description: "Run tests"}

      assert {:ok, {:tool_use, tool_calls}, _new_state} =
               ClaudeCodePlugin.execute(state, task, events.handler)

      assert length(tool_calls) == 1
      [call] = tool_calls
      assert call.name == "shell"
      assert call.args == %{"cmd" => "cargo test"}
      assert call.id == "toolu_01"

      received_events = events.collect.()
      assert Enum.any?(received_events, &match?({:tool_use, "shell", _}, &1))
    end

    test "handles rate limit (429)", %{bypass: bypass, api_base: api_base} do
      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("retry-after", "30")
        |> Plug.Conn.resp(429, "rate limited")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:error, {:rate_limited, 30}} =
               ClaudeCodePlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "handles API error (500)", %{bypass: bypass, api_base: api_base} do
      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        Plug.Conn.resp(conn, 500, "internal error")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:error, {:api_error, 500, _}} =
               ClaudeCodePlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "parses rate limit headers from response", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{
          "content" => [%{"type" => "text", "text" => "ok"}]
        })

      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("x-ratelimit-remaining-requests", "0")
        |> Plug.Conn.put_resp_header(
          "x-ratelimit-reset-requests",
          "2026-03-16T10:00:00Z"
        )
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:ok, {:complete, _}, new_state} =
               ClaudeCodePlugin.execute(state, %{description: "test"}, fn _ -> :ok end)

      assert new_state.last_rate_limit.remaining == 0
      assert new_state.last_rate_limit.limited == true
      assert %DateTime{} = new_state.last_rate_limit.reset_at
    end

    test "sends correct headers", %{bypass: bypass, api_base: api_base} do
      response_body = Jason.encode!(%{"content" => [%{"type" => "text", "text" => "ok"}]})

      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        assert Plug.Conn.get_req_header(conn, "x-api-key") == ["sk-test-key"]
        assert Plug.Conn.get_req_header(conn, "anthropic-version") == ["2023-06-01"]
        assert Plug.Conn.get_req_header(conn, "content-type") == ["application/json"]

        Plug.Conn.resp(conn, 200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test-key"})
      state = %{state | api_base: api_base}

      ClaudeCodePlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "includes tool results in messages", %{bypass: bypass, api_base: api_base} do
      response_body = Jason.encode!(%{"content" => [%{"type" => "text", "text" => "done"}]})

      Bypass.expect_once(bypass, "POST", "/v1/messages", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        request = Jason.decode!(body)
        messages = request["messages"]

        # Should have user message + tool results
        assert length(messages) >= 2

        Plug.Conn.resp(conn, 200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      task = %{
        description: "test",
        tool_results: [
          %{id: "toolu_01", result: %{status: :ok, output: "Tests passed"}}
        ]
      }

      ClaudeCodePlugin.execute(state, task, fn _ -> :ok end)
    end
  end

  describe "health/1" do
    test "returns healthy with valid state" do
      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      assert ClaudeCodePlugin.health(state) == :healthy
    end

    test "returns degraded when rate limited" do
      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      state = %{state | last_rate_limit: %{remaining: 0, reset_at: nil, limited: true}}
      assert ClaudeCodePlugin.health(state) == :degraded
    end
  end

  describe "capabilities/0" do
    test "returns coding, analysis, review" do
      assert ClaudeCodePlugin.capabilities() == [:coding, :analysis, :review]
    end
  end

  describe "rate_limit_status/1" do
    test "returns last rate limit info" do
      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      status = ClaudeCodePlugin.rate_limit_status(state)
      assert status == %{remaining: nil, reset_at: nil, limited: false}
    end
  end

  describe "disconnect/1" do
    test "returns :ok" do
      {:ok, state} = ClaudeCodePlugin.connect(%{api_key: "sk-test"})
      assert :ok = ClaudeCodePlugin.disconnect(state)
    end
  end

  # --- Helpers ---

  defp collect_events do
    pid = self()
    ref = make_ref()

    handler = fn event ->
      send(pid, {ref, event})
      :ok
    end

    collect = fn ->
      collect_messages(ref, [])
    end

    %{handler: handler, collect: collect}
  end

  defp collect_messages(ref, acc) do
    receive do
      {^ref, event} -> collect_messages(ref, [event | acc])
    after
      100 -> Enum.reverse(acc)
    end
  end
end
