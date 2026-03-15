defmodule RustyclawOrchestrator.ToolSynthesis.ApiRouter do
  @moduledoc """
  HTTP API for tool synthesis operations.

  Serves Plug.Router endpoints for Rust-side SynthToolProxy and CLI integration:
  - `GET  /api/synth/tools`       — list active synthesized tools
  - `POST /api/synth/execute`     — execute a synthesized tool
  - `POST /api/synth/synthesize`  — trigger synthesis
  - `POST /api/synth/approve`     — promote a tool
  - `POST /api/synth/suspend`     — suspend a tool
  - `DELETE /api/synth/tools/:name` — delete a tool
  """

  use Plug.Router

  alias RustyclawOrchestrator.ToolSynthesis.{
    Persistence,
    Probation,
    Registry,
    Sandbox,
    Synthesizer
  }

  plug(:match)
  plug(Plug.Parsers, parsers: [:json], json_decoder: Jason)
  plug(:dispatch)

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
end
