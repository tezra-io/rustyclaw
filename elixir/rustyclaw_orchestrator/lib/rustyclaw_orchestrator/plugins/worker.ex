defmodule RustyclawOrchestrator.Plugins.Worker do
  @moduledoc """
  GenServer managing a single plugin task execution.

  Calls ContextBuilder before `execute/3`, manages the
  execute -> tool_use -> RustBridge -> result -> re-execute loop,
  and enforces max iteration guards.

  Dispatches plugin execution to Task.Supervisor.async_nolink to keep
  the Worker GenServer responsive.
  """

  use GenServer

  alias RustyclawOrchestrator.Plugins.ContextBuilder

  require Logger

  @default_max_iterations 20
  @call_timeout 300_000
  @task_supervisor RustyclawOrchestrator.Plugins.TaskSupervisor

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

    # Dispatch to Task.Supervisor
    %Task{ref: ref} =
      Task.Supervisor.async_nolink(state.task_supervisor, fn ->
        task_loop(
          plugin_module,
          plugin_state,
          task_with_context,
          0,
          max_iterations,
          event_handler
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
