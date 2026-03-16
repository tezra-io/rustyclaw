defmodule RustyclawOrchestrator.Plugins.BatchProcessor do
  @moduledoc """
  Intelligent batch distribution of tasks across Workers.

  Groups tasks by required capability and repository, then executes:
  - Same-repo tasks: sequentially (git safety) or via worktrees
  - Different-repo tasks: in parallel via multiple Workers
  """

  alias RustyclawOrchestrator.Plugins.{AutoRouter, CronBridge, Manager}

  require Logger

  @doc """
  Submit a batch of tasks for processing.

  Groups tasks by capability and repo, then executes them with
  appropriate concurrency controls.

  Options:
  - `:git_strategy` — `:lock` (sequential) or `:worktree` (parallel via worktrees)
  - `:manager` — PluginManager server (default: Manager)
  - `:worker_supervisor` — DynamicSupervisor for Workers
  - `:max_iterations` — max tool loop iterations per task
  - `:event_handler` — callback for progress events

  Returns a list of `{task_id, result}` tuples.
  """
  @spec submit_batch([map()], keyword()) :: [{String.t(), {:ok, term()} | {:error, term()}}]
  def submit_batch(tasks, opts \\ []) do
    tasks_with_caps = Enum.map(tasks, &enrich_task/1)
    groups = group_by_repo(tasks_with_caps)

    groups
    |> Enum.map(fn {repo_path, repo_tasks} ->
      {repo_path, repo_tasks, opts}
    end)
    |> execute_groups(opts)
    |> List.flatten()
  end

  @doc "Return the number of healthy plugins available for work."
  @spec max_concurrent(keyword()) :: non_neg_integer()
  def max_concurrent(opts \\ []) do
    manager = Keyword.get(opts, :manager, Manager)
    plugins = Manager.list_plugins(server: manager)
    Enum.count(plugins, fn p -> p.status in [:healthy, :degraded] end)
  end

  # --- Internals ---

  defp enrich_task(task) do
    capabilities = task[:capabilities] || AutoRouter.route_task(task)
    Map.put(task, :capabilities, capabilities)
  end

  defp group_by_repo(tasks) do
    Enum.group_by(tasks, fn task ->
      task[:repo_path] || "no_repo"
    end)
  end

  defp execute_groups(groups, opts) do
    case length(groups) do
      1 ->
        # Single repo group — run sequentially or with worktrees
        [{_repo, tasks, group_opts}] = groups
        execute_repo_group(tasks, Keyword.merge(group_opts, opts))

      _ ->
        # Multiple repos — parallelize across repos
        groups
        |> Enum.map(&async_repo_group(&1, opts))
        |> Task.await_many(Keyword.get(opts, :timeout, 600_000))
    end
  end

  defp async_repo_group({_repo, tasks, group_opts}, opts) do
    merged_opts = Keyword.merge(group_opts, opts)
    Task.async(fn -> execute_repo_group(tasks, merged_opts) end)
  end

  defp execute_repo_group(tasks, opts) do
    git_strategy = Keyword.get(opts, :git_strategy, :lock)

    case git_strategy do
      :worktree ->
        execute_parallel_worktree(tasks, opts)

      _ ->
        execute_sequential(tasks, opts)
    end
  end

  defp execute_sequential(tasks, opts) do
    Enum.map(tasks, fn task ->
      identifier = task[:identifier] || task[:id] || "unknown"
      Logger.info("BatchProcessor: processing #{identifier} sequentially")
      result = CronBridge.submit_coding_task(task, opts)
      {identifier, result}
    end)
  end

  defp execute_parallel_worktree(tasks, opts) do
    tasks
    |> Enum.map(fn task ->
      Task.async(fn ->
        identifier = task[:identifier] || task[:id] || "unknown"
        Logger.info("BatchProcessor: processing #{identifier} in worktree")

        task_opts = Keyword.merge(opts, git_strategy: :worktree)
        result = CronBridge.submit_coding_task(task, task_opts)
        {identifier, result}
      end)
    end)
    |> Task.await_many(Keyword.get(opts, :timeout, 600_000))
  end
end
