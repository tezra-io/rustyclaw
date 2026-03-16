defmodule RustyclawOrchestrator.Plugins.PluginRouter do
  @moduledoc """
  HTTP API for plugin management and task execution.

  Endpoints:
  - `GET  /api/plugins`                  — list all plugins with status
  - `POST /api/plugins/exec`             — submit a task for execution
  - `POST /api/plugins/:name/retry/:task_id` — retry a specific task
  - `GET  /api/plugins/status`           — active workers and pending retries
  """

  use Plug.Router

  alias RustyclawOrchestrator.Plugins.{Manager, RetryScheduler, Worker}

  plug(:match)
  plug(Plug.Parsers, parsers: [:json], json_decoder: Jason)
  plug(:dispatch)

  # --- GET /api/plugins ---

  get "/api/plugins" do
    plugins = Manager.list_plugins()
    json_response(conn, 200, %{ok: true, plugins: plugins})
  end

  # --- POST /api/plugins/exec ---

  post "/api/plugins/exec" do
    dispatch_task(conn)
  end

  # --- POST /api/plugins/:name/retry/:task_id ---

  post "/api/plugins/:name/retry/:task_id" do
    task = %{
      id: task_id,
      description: Map.get(conn.body_params, "description", "retry"),
      capabilities: [:coding],
      retry_attempt: 0
    }

    retry_reason = Map.get(conn.body_params, "reason", "manual_retry") |> String.to_atom()

    case RetryScheduler.schedule_retry(task, retry_reason, name) do
      :ok ->
        json_response(conn, 200, %{ok: true, message: "Retry scheduled for #{task_id}"})

      {:error, err} ->
        json_response(conn, 422, %{ok: false, error: inspect(err)})
    end
  end

  # --- GET /api/plugins/status ---

  get "/api/plugins/status" do
    plugins = Manager.list_plugins()
    pending_retries = RetryScheduler.pending_count()

    status = %{
      ok: true,
      plugins: plugins,
      pending_retries: pending_retries
    }

    json_response(conn, 200, status)
  end

  # --- Catch-all ---

  match _ do
    json_response(conn, 404, %{error: "not found"})
  end

  # --- Helpers ---

  defp dispatch_task(conn) do
    with {:ok, capability} <- require_field(conn.body_params, "capability"),
         {:ok, description} <- require_field(conn.body_params, "description"),
         {:ok, cap_atom} <- safe_to_atom(capability),
         {:ok, plugin} <- find_plugin(cap_atom, capability) do
      submit_task(conn, plugin, description, cap_atom)
    else
      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})

      {:error, :unknown_atom} ->
        json_response(conn, 400, %{ok: false, error: "unknown capability"})

      {:error, {:no_plugin, msg}} ->
        json_response(conn, 404, %{ok: false, error: msg})
    end
  end

  defp find_plugin(cap_atom, capability) do
    case Manager.plugins_for_capabilities([cap_atom]) do
      [plugin | _] -> {:ok, plugin}
      [] -> {:error, {:no_plugin, "No available plugin for capability: #{capability}"}}
    end
  end

  defp submit_task(conn, plugin, description, cap_atom) do
    task = %{
      id: "task-#{System.unique_integer([:positive])}",
      description: description,
      capabilities: [cap_atom],
      repo_path: Map.get(conn.body_params, "repo_path"),
      git_strategy: parse_git_strategy(conn.body_params)
    }

    case DynamicSupervisor.start_child(
           RustyclawOrchestrator.Plugins.WorkerSupervisor,
           {Worker, []}
         ) do
      {:ok, worker} ->
        spawn(fn ->
          Worker.execute(worker,
            plugin_module: plugin.module,
            plugin_state: plugin.state,
            task: task
          )
        end)

        json_response(conn, 202, %{
          ok: true,
          task_id: task.id,
          plugin: plugin.name,
          message: "Task submitted"
        })

      {:error, reason} ->
        json_response(conn, 500, %{ok: false, error: inspect(reason)})
    end
  end

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

  defp safe_to_atom(string) do
    {:ok, String.to_existing_atom(string)}
  rescue
    ArgumentError -> {:error, :unknown_atom}
  end

  defp parse_git_strategy(params) do
    case Map.get(params, "git_strategy") do
      "worktree" -> :worktree
      _ -> :lock
    end
  end
end
