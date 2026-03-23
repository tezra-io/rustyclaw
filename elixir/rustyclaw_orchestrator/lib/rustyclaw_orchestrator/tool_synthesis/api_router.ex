defmodule RustyclawOrchestrator.ToolSynthesis.ApiRouter do
  @moduledoc """
  HTTP API for tool synthesis operations.

  Serves Plug.Router endpoints for Rust-side SynthToolProxy and CLI integration:
  - `GET  /api/synth/tools`          — list active synthesized tools
  - `POST /api/synth/execute`        — execute a synthesized tool
  - `POST /api/synth/synthesize`     — trigger synthesis
  - `POST /api/synth/approve`        — promote a tool
  - `POST /api/synth/suspend`        — suspend a tool
  - `POST /api/synth/improve`        — request tool improvement
  - `POST /api/synth/rollback`       — rollback to previous version
  - `GET  /api/synth/versions/:name` — list version history
  - `DELETE /api/synth/tools/:name`  — delete a tool
  """

  use Plug.Router

  alias RustyclawOrchestrator.ToolSynthesis.{
    Improver,
    Persistence,
    Probation,
    Registry,
    Sandbox,
    Synthesizer
  }

  plug(:match)
  plug(Plug.Parsers, parsers: [:json], json_decoder: Jason)
  plug(:dispatch)

  # --- GET /health ---

  get "/health" do
    send_resp(conn, 200, Jason.encode!(%{status: "ok"}))
  end

  # --- GET /api/synth/tools ---

  get "/api/synth/tools" do
    tools =
      Registry.list()
      |> Enum.map(fn entry ->
        %{
          name: entry.name,
          description: entry.description,
          parameters_schema: entry.parameters_schema,
          status: entry.status,
          invocation_count: entry.invocation_count,
          success_count: entry.success_count
        }
      end)

    json_response(conn, 200, tools)
  end

  # --- POST /api/synth/execute ---

  post "/api/synth/execute" do
    with {:ok, tool_name} <- require_field(conn.body_params, "tool"),
         {:ok, params} <- require_field(conn.body_params, "params"),
         {:ok, entry} <- Registry.lookup(tool_name) do
      if entry.status in [:probation, :promoted] do
        start_time = System.monotonic_time(:millisecond)
        result = Sandbox.execute(entry.module, params)
        elapsed = System.monotonic_time(:millisecond) - start_time

        {success, crash} =
          case result do
            {:ok, _} -> {true, false}
            {:error, msg} -> {false, String.contains?(msg, ["crashed", "timed out"])}
          end

        Registry.update_metrics(tool_name, success, elapsed)
        Probation.record_invocation(tool_name, success, crash: crash, latency_ms: elapsed)

        case result do
          {:ok, output} ->
            json_response(conn, 200, %{ok: true, output: output})

          {:error, reason} ->
            json_response(conn, 200, %{ok: false, error: reason})
        end
      else
        json_response(conn, 400, %{ok: false, error: "tool is #{entry.status}, not executable"})
      end
    else
      {:error, :not_found} ->
        json_response(conn, 404, %{ok: false, error: "tool not found"})

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- POST /api/synth/synthesize ---

  post "/api/synth/synthesize" do
    with {:ok, capability} <- require_field(conn.body_params, "capability"),
         {:ok, suggested_name} <- require_field(conn.body_params, "suggested_name") do
      request = %{capability: capability, suggested_name: suggested_name}

      opts =
        []
        |> maybe_add_opt(conn.body_params, "agent_id", :agent_id)
        |> maybe_add_opt(conn.body_params, "input_example", :input_example)
        |> maybe_add_opt(conn.body_params, "expected_output", :expected_output)

      case Synthesizer.synthesize(request, opts) do
        {:ok, tool_info} ->
          json_response(conn, 200, %{
            ok: true,
            tool: %{
              name: tool_info.name,
              module: inspect(tool_info.module),
              status: tool_info.status
            }
          })

        {:error, reason} ->
          json_response(conn, 200, %{ok: false, error: inspect(reason)})
      end
    else
      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- POST /api/synth/approve ---

  post "/api/synth/approve" do
    case require_field(conn.body_params, "name") do
      {:ok, name} ->
        case Registry.update_status(name, :promoted) do
          :ok -> json_response(conn, 200, %{ok: true})
          {:error, :not_found} -> json_response(conn, 404, %{ok: false, error: "tool not found"})
        end

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- POST /api/synth/suspend ---

  post "/api/synth/suspend" do
    case require_field(conn.body_params, "name") do
      {:ok, name} ->
        case Registry.update_status(name, :suspended) do
          :ok -> json_response(conn, 200, %{ok: true})
          {:error, :not_found} -> json_response(conn, 404, %{ok: false, error: "tool not found"})
        end

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- DELETE /api/synth/tools/:name ---

  delete "/api/synth/tools/:name" do
    case Registry.lookup(name) do
      {:ok, _entry} ->
        Registry.unload(name)
        Persistence.delete(name)
        json_response(conn, 200, %{ok: true})

      {:error, :not_found} ->
        json_response(conn, 404, %{ok: false, error: "tool not found"})
    end
  end

  # --- POST /api/synth/improve ---

  post "/api/synth/improve" do
    case require_field(conn.body_params, "name") do
      {:ok, name} ->
        opts =
          []
          |> maybe_add_opt(conn.body_params, "failure_input", :failure_input)
          |> maybe_add_opt(conn.body_params, "expected_output", :expected_output)
          |> maybe_add_opt(conn.body_params, "error", :error_message)

        case Improver.improve(name, opts) do
          {:ok, result} ->
            json_response(conn, 200, %{
              ok: true,
              tool: %{name: result.name, version: result.version}
            })

          {:error, reason} ->
            json_response(conn, 200, %{ok: false, error: inspect(reason)})
        end

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- POST /api/synth/rollback ---

  post "/api/synth/rollback" do
    case require_field(conn.body_params, "name") do
      {:ok, name} ->
        case Improver.rollback(name) do
          :ok -> json_response(conn, 200, %{ok: true})
          {:error, reason} -> json_response(conn, 200, %{ok: false, error: inspect(reason)})
        end

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- GET /api/synth/versions/:name ---

  get "/api/synth/versions/:name" do
    case Improver.versions(name) do
      {:ok, versions} -> json_response(conn, 200, %{ok: true, versions: versions})
      {:error, reason} -> json_response(conn, 200, %{ok: false, error: inspect(reason)})
    end
  end

  # --- POST /api/messages/inbound (Rust → Elixir bridge) ---

  post "/api/messages/inbound" do
    case require_bridge_secret(conn) do
      :ok ->
        agent_name = Map.get(conn.body_params, "agent_name", "default")
        message = Map.get(conn.body_params, "message", "")
        channel_info = Map.get(conn.body_params, "channel_info", %{})

        if message == "" do
          json_response(conn, 400, %{ok: false, error: "message is required"})
        else
          opts = [channel_info: channel_info]

          case RustyclawOrchestrator.BtwRouter.route(agent_name, message, opts) do
            {:btw, pid} ->
              json_response(conn, 200, %{ok: true, routed: "btw", pid: inspect(pid)})

            {:main, :ok} ->
              json_response(conn, 200, %{ok: true, routed: "main"})

            {:error, reason} ->
              json_response(conn, 500, %{ok: false, error: inspect(reason)})
          end
        end

      {:error, conn} ->
        conn
    end
  end

  # --- Catch-all ---

  match _ do
    json_response(conn, 404, %{error: "not found"})
  end

  # --- Helpers ---

  defp json_response(conn, status, body) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(status, Jason.encode!(body))
  end

  defp require_field(params, field) do
    case Map.fetch(params, field) do
      {:ok, value} -> {:ok, value}
      :error -> {:error, {:missing_field, field}}
    end
  end

  defp maybe_add_opt(opts, params, json_key, opt_key) do
    case Map.fetch(params, json_key) do
      {:ok, value} -> Keyword.put(opts, opt_key, value)
      :error -> opts
    end
  end

  defp require_bridge_secret(conn) do
    expected = System.get_env("RUSTYCLAW_BRIDGE_SECRET") || ""

    if expected == "" do
      # No secret configured — reject all bridge calls as a safety default
      {:error, json_response(conn, 403, %{ok: false, error: "bridge secret not configured"})}
    else
      provided =
        conn
        |> Plug.Conn.get_req_header("x-bridge-secret")
        |> List.first("")

      if Plug.Crypto.secure_compare(provided, expected) do
        :ok
      else
        {:error, json_response(conn, 401, %{ok: false, error: "unauthorized"})}
      end
    end
  end
end
