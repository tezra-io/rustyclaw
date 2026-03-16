defmodule RustyclawOrchestrator.Plugins.PluginRouter do
  @moduledoc """
  HTTP API for plugin management and task execution.

  Endpoints:
  - `GET  /api/plugins`                      — list all plugins with status
  - `POST /api/plugins/exec`                 — submit a task for execution
  - `POST /api/plugins/:name/retry/:task_id` — retry a specific task
  - `GET  /api/plugins/status`               — active workers and pending retries
  - `POST /api/plugins/sessions`             — start a dev session
  - `GET  /api/plugins/sessions/:id`         — session status
  - `DELETE /api/plugins/sessions/:id`       — cancel a session
  - `GET  /api/plugins/queue`               — queue status and contents
  - `POST /api/plugins/queue/batch`          — submit batch of tasks
  - `DELETE /api/plugins/queue/:task_id`     — remove task from queue
  """

  use Plug.Router

  alias RustyclawOrchestrator.Plugins.{
    BatchProcessor,
    Manager,
    RetryScheduler,
    TaskOrchestrator,
    TaskQueue,
    Worker
  }

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

  # --- POST /api/plugins/sessions ---

  post "/api/plugins/sessions" do
    with {:ok, repo_path} <- require_field(conn.body_params, "repo_path"),
         {:ok, issues} <- require_field(conn.body_params, "issues") do
      config = %{
        repo_path: repo_path,
        issues: issues,
        quality_gates: Map.get(conn.body_params, "quality_gates", []),
        plugin_opts: parse_plugin_opts(conn.body_params)
      }

      case TaskOrchestrator.start_session(config) do
        {:ok, session_id} ->
          json_response(conn, 201, %{ok: true, session_id: session_id})

        {:error, reason} ->
          json_response(conn, 422, %{ok: false, error: inspect(reason)})
      end
    else
      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- GET /api/plugins/sessions/:id ---

  get "/api/plugins/sessions/:id" do
    case TaskOrchestrator.get_status(id) do
      {:ok, status} ->
        json_response(conn, 200, %{ok: true, session: status})

      {:error, :not_found} ->
        json_response(conn, 404, %{ok: false, error: "session not found"})
    end
  end

  # --- DELETE /api/plugins/sessions/:id ---

  delete "/api/plugins/sessions/:id" do
    case TaskOrchestrator.cancel_session(id) do
      :ok ->
        json_response(conn, 200, %{ok: true, message: "session cancelled"})

      {:error, :not_found} ->
        json_response(conn, 404, %{ok: false, error: "session not found"})
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

  # --- GET /api/plugins/queue ---

  get "/api/plugins/queue" do
    status = TaskQueue.status()
    tasks = TaskQueue.list_tasks()

    json_response(conn, 200, %{ok: true, status: status, tasks: tasks})
  end

  # --- POST /api/plugins/queue/batch ---

  post "/api/plugins/queue/batch" do
    case require_field(conn.body_params, "tasks") do
      {:ok, tasks} ->
        opts = parse_plugin_opts(conn.body_params)
        batch_id = "batch-#{System.unique_integer([:positive])}"

        spawn(fn ->
          BatchProcessor.submit_batch(tasks, opts)
        end)

        json_response(conn, 202, %{ok: true, batch_id: batch_id, task_count: length(tasks)})

      {:error, {:missing_field, field}} ->
        json_response(conn, 400, %{ok: false, error: "missing field: #{field}"})
    end
  end

  # --- DELETE /api/plugins/queue/:task_id ---

  delete "/api/plugins/queue/:task_id" do
    case TaskQueue.remove_task(task_id) do
      :ok ->
        json_response(conn, 200, %{ok: true, message: "task removed"})

      {:error, :not_found} ->
        json_response(conn, 404, %{ok: false, error: "task not found in queue"})
    end
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

  defp parse_plugin_opts(params) do
    opts = []

    opts =
      if params["git_strategy"],
        do: [{:git_strategy, parse_git_strategy(params)} | opts],
        else: opts

    opts =
      if params["max_iterations"],
        do: [{:max_iterations, params["max_iterations"]} | opts],
        else: opts

    opts
  end
end
