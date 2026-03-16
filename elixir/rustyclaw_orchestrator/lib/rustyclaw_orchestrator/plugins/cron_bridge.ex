defmodule RustyclawOrchestrator.Plugins.CronBridge do
  @moduledoc """
  Bridge between the cron system and the plugin system.

  Submits Linear issues as coding tasks to the plugin system,
  handling single and batch task submission with git safety
  (sequential execution — one issue at a time).
  """

  alias RustyclawOrchestrator.Plugins.{Manager, Worker}

  require Logger

  @worker_supervisor RustyclawOrchestrator.Plugins.WorkerSupervisor

  @doc """
  Submit a single coding task derived from a Linear issue.

  The issue map should contain:
  - `:identifier` — e.g. "TEZ-250"
  - `:title` — issue title
  - `:description` — issue description/body
  - `:repo_path` — path to the git repository

  Options:
  - `:capabilities` — capabilities to match (default: `[:coding]`)
  - `:git_strategy` — `:lock` or `:worktree` (default: `:lock`)
  - `:max_iterations` — max tool loop iterations (default: 20)
  - `:event_handler` — callback for progress events
  - `:manager` — PluginManager server (default: Manager)
  """
  @spec submit_coding_task(map(), keyword()) :: {:ok, term()} | {:error, term()}
  def submit_coding_task(issue, opts \\ []) do
    capabilities = Keyword.get(opts, :capabilities, [:coding])
    manager = Keyword.get(opts, :manager, Manager)

    case Manager.plugins_for_capabilities(capabilities, server: manager) do
      [plugin | _] ->
        execute_with_plugin(plugin, issue, opts)

      [] ->
        {:error, :no_available_plugin}
    end
  end

  @doc """
  Submit a batch of issues sequentially (one at a time for git safety).

  Returns a list of `{identifier, result}` tuples. Continues to the
  next issue on failure — does not abort the batch.
  """
  @spec submit_batch([map()], keyword()) :: [{String.t(), {:ok, term()} | {:error, term()}}]
  def submit_batch(issues, opts \\ []) do
    Enum.map(issues, fn issue ->
      identifier = issue[:identifier] || issue["identifier"] || "unknown"
      Logger.info("CronBridge: processing issue #{identifier}")
      result = submit_coding_task(issue, opts)

      case result do
        {:ok, _} ->
          Logger.info("CronBridge: issue #{identifier} completed successfully")

        {:error, reason} ->
          Logger.warning("CronBridge: issue #{identifier} failed: #{inspect(reason)}")
      end

      {identifier, result}
    end)
  end

  defp execute_with_plugin(plugin, issue, opts) do
    task = build_task(issue, opts)
    worker_supervisor = Keyword.get(opts, :worker_supervisor, @worker_supervisor)

    case DynamicSupervisor.start_child(worker_supervisor, {Worker, []}) do
      {:ok, worker} ->
        result = run_worker(worker, plugin, task, opts)
        DynamicSupervisor.terminate_child(worker_supervisor, worker)
        result

      {:error, reason} ->
        {:error, {:worker_start_failed, reason}}
    end
  end

  defp build_task(issue, opts) do
    identifier = issue[:identifier] || issue["identifier"] || "unknown"
    title = issue[:title] || issue["title"] || ""
    description = issue[:description] || issue["description"] || ""

    %{
      id: "cron-#{identifier}-#{System.unique_integer([:positive])}",
      description: "#{identifier}: #{title}\n\n#{description}",
      capabilities: [:coding],
      repo_path: issue[:repo_path] || issue["repo_path"],
      git_strategy: Keyword.get(opts, :git_strategy, :lock),
      source: :cron_bridge,
      linear_issue: identifier
    }
  end

  defp run_worker(worker, plugin, task, opts) do
    Worker.execute(worker,
      plugin_module: plugin.module,
      plugin_state: plugin.state,
      task: task,
      max_iterations: Keyword.get(opts, :max_iterations, 20),
      event_handler: Keyword.get(opts, :event_handler, fn _event -> :ok end)
    )
  end
end
