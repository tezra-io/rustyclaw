defmodule RustyclawOrchestrator.Plugins.Worker do
  @moduledoc """
  GenServer managing a single plugin task execution.

  Calls ContextBuilder before `execute/3`, manages the
  execute -> tool_use -> RustBridge -> result -> re-execute loop,
  and enforces max iteration guards.

  For coding tasks, acquires a ResourceLock on the repo path before execution.
  When `git_strategy: :worktree` is set, creates an isolated git worktree instead.

  Dispatches plugin execution to Task.Supervisor.async_nolink to keep
  the Worker GenServer responsive.
  """

  use GenServer

  alias RustyclawOrchestrator.Plugins.{ContextBuilder, GitWorktree}
  alias RustyclawOrchestrator.ResourceLock

  require Logger

  @default_max_iterations 20
  @call_timeout 300_000
  @task_supervisor RustyclawOrchestrator.Plugins.TaskSupervisor
  @lock_wait_ms 10_000

  @type event_callback :: (term() -> :ok)

  # --- Client API ---

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts)
  end

  @doc """
  Execute a task using the given plugin.

  Options:
  - `:plugin_module` — the plugin module implementing Behaviour
  - `:plugin_state` — current plugin state from connect/1
  - `:task` — task map with at least `:description`
  - `:max_iterations` — max tool_use loop iterations (default 20)
  - `:event_handler` — function called with events (default: no-op)
  - `:task_supervisor` — Task.Supervisor name (default: Plugins.TaskSupervisor)
  """
  @spec execute(pid(), keyword()) :: {:ok, term()} | {:error, term()}
  def execute(worker, opts) do
    GenServer.call(worker, {:execute, opts}, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    state = %{
      pending: nil,
      task_ref: nil,
      task_supervisor: Keyword.get(opts, :task_supervisor, @task_supervisor)
    }

    {:ok, state}
  end

  @impl true
  def handle_call({:execute, opts}, from, %{pending: nil} = state) do
    plugin_module = Keyword.fetch!(opts, :plugin_module)
    plugin_state = Keyword.fetch!(opts, :plugin_state)
    task = Keyword.fetch!(opts, :task)
    max_iterations = Keyword.get(opts, :max_iterations, @default_max_iterations)
    event_handler = Keyword.get(opts, :event_handler, fn _event -> :ok end)

    # Build context before execution
    capabilities = plugin_module.capabilities()
    context = ContextBuilder.build(task, capabilities)
    task_with_context = Map.put(task, :context, context)

    is_coding = :coding in capabilities
    git_strategy = Map.get(task, :git_strategy, :lock)

    # Dispatch to Task.Supervisor
    %Task{ref: ref} =
      Task.Supervisor.async_nolink(state.task_supervisor, fn ->
        run_with_concurrency_control(
          plugin_module,
          plugin_state,
          task_with_context,
          max_iterations,
          event_handler,
          is_coding,
          git_strategy
        )
      end)

    {:noreply, %{state | pending: from, task_ref: ref}}
  end

  def handle_call({:execute, _opts}, _from, state) do
    {:reply, {:error, :busy}, state}
  end

  @impl true
  def handle_info({ref, result}, %{task_ref: ref} = state) when is_reference(ref) do
    Process.demonitor(ref, [:flush])

    if state.pending do
      GenServer.reply(state.pending, result)
    end

    {:noreply, %{state | pending: nil, task_ref: nil}}
  end

  def handle_info({:DOWN, ref, :process, _pid, reason}, %{task_ref: ref} = state) do
    if state.pending do
      GenServer.reply(state.pending, {:error, {:worker_crashed, reason}})
    end

    {:noreply, %{state | pending: nil, task_ref: nil}}
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Concurrency Control ---

  defp run_with_concurrency_control(
         plugin_module,
         plugin_state,
         task,
         max_iterations,
         event_handler,
         true = _is_coding,
         :worktree
       ) do
    repo_path = Map.get(task, :repo_path)

    if repo_path do
      worker_id = "#{System.unique_integer([:positive])}"

      case GitWorktree.create_worktree(repo_path, worker_id) do
        {:ok, worktree_path} ->
          # Execute in the worktree directory
          task_in_worktree = Map.put(task, :repo_path, worktree_path)

          try do
            task_loop(
              plugin_module,
              plugin_state,
              task_in_worktree,
              0,
              max_iterations,
              event_handler
            )
          after
            GitWorktree.cleanup_worktree(worktree_path)
          end

        {:error, reason} ->
          Logger.error("Worktree creation failed: #{reason}, falling back to lock strategy")

          run_with_lock(
            plugin_module,
            plugin_state,
            task,
            max_iterations,
            event_handler,
            repo_path
          )
      end
    else
      task_loop(plugin_module, plugin_state, task, 0, max_iterations, event_handler)
    end
  end

  defp run_with_concurrency_control(
         plugin_module,
         plugin_state,
         task,
         max_iterations,
         event_handler,
         true = _is_coding,
         _lock_strategy
       ) do
    repo_path = Map.get(task, :repo_path)

    if repo_path do
      run_with_lock(plugin_module, plugin_state, task, max_iterations, event_handler, repo_path)
    else
      task_loop(plugin_module, plugin_state, task, 0, max_iterations, event_handler)
    end
  end

  defp run_with_concurrency_control(
         plugin_module,
         plugin_state,
         task,
         max_iterations,
         event_handler,
         false = _is_coding,
         _strategy
       ) do
    task_loop(plugin_module, plugin_state, task, 0, max_iterations, event_handler)
  end

  defp run_with_lock(plugin_module, plugin_state, task, max_iterations, event_handler, repo_path) do
    lock_key = "repo:#{repo_path}"

    case ResourceLock.acquire(lock_key, wait_ms: @lock_wait_ms, priority: :main) do
      :ok ->
        try do
          task_loop(plugin_module, plugin_state, task, 0, max_iterations, event_handler)
        after
          ResourceLock.release(lock_key)
        end

      {:error, :resource_busy} ->
        {:error, :repo_locked}
    end
  end

  # --- Task Loop ---

  defp task_loop(_module, _state, _task, iteration, max_iterations, _handler)
       when iteration >= max_iterations do
    {:error, {:max_iterations, iteration}}
  end

  defp task_loop(plugin_module, plugin_state, task, iteration, max_iterations, event_handler) do
    case plugin_module.execute(plugin_state, task, event_handler) do
      {:ok, {:tool_use, tool_calls}, new_state} ->
        event_handler.({:tool_use_batch, tool_calls})
        results = execute_tools(tool_calls)
        event_handler.({:tool_results_batch, results})
        updated_task = Map.put(task, :tool_results, results)

        task_loop(
          plugin_module,
          new_state,
          updated_task,
          iteration + 1,
          max_iterations,
          event_handler
        )

      {:ok, {:complete, result}, _new_state} ->
        {:ok, result}

      {:error, {:rate_limited, retry_after}} ->
        {:error, {:rate_limited, retry_after}}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp execute_tools(tool_calls) do
    Enum.map(tool_calls, fn call ->
      name = call[:name] || call["name"]
      args = call[:args] || call["args"] || %{}

      result =
        try do
          case RustyclawOrchestrator.RustBridge.run_task("system", "tool_exec",
                 tool_name: name,
                 tool_args: args
               ) do
            {:ok, output} -> %{status: :ok, output: output}
            {:error, reason} -> %{status: :error, error: inspect(reason)}
          end
        rescue
          e -> %{status: :error, error: Exception.message(e)}
        catch
          :exit, reason -> %{status: :error, error: inspect(reason)}
        end

      Map.merge(call, %{result: result})
    end)
  end
end
