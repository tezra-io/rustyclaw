defmodule RustyclawOrchestrator.Plugins.CodexPluginTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.CodexPlugin

  describe "connect/1" do
    test "succeeds with valid API key" do
      assert {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test-123"})
      assert state.api_key == "sk-test-123"
      assert state.model == "codex-mini-latest"
      assert state.max_tokens == 16_384
      assert state.messages == []
    end

    test "accepts custom model" do
      assert {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test", model: "gpt-4o"})
      assert state.model == "gpt-4o"
    end

    test "fails with missing API key" do
      assert {:error, :missing_api_key} = CodexPlugin.connect(%{})
    end

    test "fails with empty API key" do
      assert {:error, :missing_api_key} = CodexPlugin.connect(%{api_key: ""})
    end
  end

  describe "execute/3 with Bypass" do
    setup do
      bypass = Bypass.open()
      {:ok, bypass: bypass, api_base: "http://localhost:#{bypass.port}/v1/responses"}
    end

    test "handles complete response (OpenAI chat format)", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{
          "id" => "chatcmpl-123",
          "choices" => [
            %{
              "index" => 0,
              "message" => %{
                "role" => "assistant",
                "content" => "Here is the fix."
              },
              "finish_reason" => "stop"
            }
          ]
        })

      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("content-type", "application/json")
        |> Plug.Conn.put_resp_header("x-ratelimit-remaining-requests", "100")
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      events = collect_events()
      task = %{description: "Fix the bug"}

      assert {:ok, {:complete, result}, new_state} =
               CodexPlugin.execute(state, task, events.handler)

      assert result.output == "Here is the fix."
      assert new_state.last_rate_limit.remaining == 100
    end

    test "handles tool_calls response (OpenAI format)", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{
          "id" => "chatcmpl-456",
          "choices" => [
            %{
              "index" => 0,
              "message" => %{
                "role" => "assistant",
                "content" => nil,
                "tool_calls" => [
                  %{
                    "id" => "call_abc",
                    "type" => "function",
                    "function" => %{
                      "name" => "shell",
                      "arguments" => Jason.encode!(%{"cmd" => "npm test"})
                    }
                  }
                ]
              },
              "finish_reason" => "tool_calls"
            }
          ]
        })

      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("content-type", "application/json")
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      events = collect_events()
      task = %{description: "Run tests"}

      assert {:ok, {:tool_use, tool_calls}, _new_state} =
               CodexPlugin.execute(state, task, events.handler)

      assert length(tool_calls) == 1
      [call] = tool_calls
      assert call.name == "shell"
      assert call.args == %{"cmd" => "npm test"}
      assert call.id == "call_abc"

      received_events = events.collect.()
      assert Enum.any?(received_events, &match?({:tool_use, "shell", _}, &1))
    end

    test "handles rate limit (429)", %{bypass: bypass, api_base: api_base} do
      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("retry-after", "45")
        |> Plug.Conn.resp(429, "rate limited")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:error, {:rate_limited, 45}} =
               CodexPlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "handles API error (503)", %{bypass: bypass, api_base: api_base} do
      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        Plug.Conn.resp(conn, 503, "service unavailable")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:error, {:api_error, 503, _}} =
               CodexPlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "sends correct headers with Bearer token", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{"choices" => [%{"message" => %{"content" => "ok"}}]})

      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        assert Plug.Conn.get_req_header(conn, "authorization") == ["Bearer sk-mykey"]
        assert Plug.Conn.get_req_header(conn, "content-type") == ["application/json"]

        Plug.Conn.resp(conn, 200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-mykey"})
      state = %{state | api_base: api_base}

      CodexPlugin.execute(state, %{description: "test"}, fn _ -> :ok end)
    end

    test "includes tool results as tool messages", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{"choices" => [%{"message" => %{"content" => "done"}}]})

      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        request = Jason.decode!(body)
        messages = request["messages"]

        # Should have user message + tool result messages
        tool_msgs = Enum.filter(messages, &(&1["role"] == "tool"))
        assert [_ | _] = tool_msgs

        Plug.Conn.resp(conn, 200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      task = %{
        description: "test",
        tool_results: [
          %{id: "call_abc", result: %{status: :ok, output: "Tests passed"}}
        ]
      }

      CodexPlugin.execute(state, task, fn _ -> :ok end)
    end

    test "parses rate limit headers", %{bypass: bypass, api_base: api_base} do
      response_body =
        Jason.encode!(%{"choices" => [%{"message" => %{"content" => "ok"}}]})

      Bypass.expect_once(bypass, "POST", "/v1/responses", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("x-ratelimit-remaining-requests", "0")
        |> Plug.Conn.put_resp_header(
          "x-ratelimit-reset-requests",
          "2026-03-16T12:00:00Z"
        )
        |> Plug.Conn.resp(200, "data: #{response_body}\n\ndata: [DONE]\n\n")
      end)

      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | api_base: api_base}

      assert {:ok, {:complete, _}, new_state} =
               CodexPlugin.execute(state, %{description: "test"}, fn _ -> :ok end)

      assert new_state.last_rate_limit.remaining == 0
      assert new_state.last_rate_limit.limited == true
    end
  end

  describe "health/1" do
    test "returns healthy with valid state" do
      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      assert CodexPlugin.health(state) == :healthy
    end

    test "returns degraded when rate limited" do
      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      state = %{state | last_rate_limit: %{remaining: 0, reset_at: nil, limited: true}}
      assert CodexPlugin.health(state) == :degraded
    end
  end

  describe "capabilities/0" do
    test "returns coding" do
      assert CodexPlugin.capabilities() == [:coding]
    end
  end

  describe "rate_limit_status/1" do
    test "returns last rate limit info" do
      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      status = CodexPlugin.rate_limit_status(state)
      assert status == %{remaining: nil, reset_at: nil, limited: false}
    end
  end

  describe "disconnect/1" do
    test "returns :ok" do
      {:ok, state} = CodexPlugin.connect(%{api_key: "sk-test"})
      assert :ok = CodexPlugin.disconnect(state)
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
