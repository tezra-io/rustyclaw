defmodule RustyclawOrchestrator.Plugins.RetryScheduler do
  @moduledoc """
  GenServer that schedules task retries with exponential backoff and fallback routing.

  Handles two retry categories:
  - **Rate limited**: waits until the plugin's `reset_at` time before requeuing
  - **Transient failures**: exponential backoff (1s → 2s → 4s → 8s, max 60s), max 5 retries

  When retries are exhausted, attempts fallback routing via PluginManager to find
  an alternative plugin with matching capabilities.
  """

  use GenServer

  alias RustyclawOrchestrator.Plugins.Manager

  require Logger

  @max_retries 5
  @base_delay_ms 1_000
  @max_delay_ms 60_000

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, _init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, opts, name: name)
  end

  @doc """
  Schedule a retry for a failed task.

  ## Options
    - `:manager` — PluginManager server (default: Manager)
    - `:callback` — function called with `{:retry, task, plugin_name}` or `{:exhausted, task}`
  """
  @spec schedule_retry(map(), atom() | tuple(), String.t(), keyword()) :: :ok | {:error, term()}
  def schedule_retry(task, reason, plugin_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:schedule_retry, task, reason, plugin_name, opts})
  end

  @doc "Get pending retry count."
  @spec pending_count(keyword()) :: non_neg_integer()
  def pending_count(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :pending_count)
  end

  @doc "List all pending retries."
  @spec list_pending(keyword()) :: [map()]
  def list_pending(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :list_pending)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    state = %{
      pending: %{},
      callback: Keyword.get(opts, :callback),
      manager: Keyword.get(opts, :manager, Manager),
      next_id: 1
    }

    {:ok, state}
  end

  @impl true
  def handle_call({:schedule_retry, task, reason, plugin_name, opts}, _from, state) do
    task_id = task[:id] || "retry-#{state.next_id}"
    attempt = Map.get(task, :retry_attempt, 0) + 1
    callback = Keyword.get(opts, :callback, state.callback)

    do_schedule(state, task, task_id, attempt, reason, plugin_name, callback)
  end

  def handle_call(:pending_count, _from, state) do
    {:reply, map_size(state.pending), state}
  end

  def handle_call(:list_pending, _from, state) do
    entries =
      Enum.map(state.pending, fn {id, entry} ->
        %{
          id: id,
          plugin_name: entry.plugin_name,
          attempt: entry.attempt,
          reason: entry.reason,
          delay_ms: entry.delay_ms
        }
      end)

    {:reply, entries, state}
  end

  @impl true
  def handle_info({:fire_retry, task_id}, state) do
    case Map.pop(state.pending, task_id) do
      {nil, _} ->
        {:noreply, state}

      {entry, pending} ->
        if entry.callback, do: entry.callback.({:retry, entry.task, entry.plugin_name})
        Logger.info("Firing retry for task #{task_id} (attempt #{entry.attempt})")
        {:noreply, %{state | pending: pending}}
    end
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Internals ---

  defp do_schedule(state, task, task_id, attempt, _reason, plugin_name, callback)
       when attempt > @max_retries do
    reply = handle_exhausted(task, task_id, plugin_name, state.manager, callback)
    {:reply, reply, %{state | next_id: state.next_id + 1}}
  end

  defp do_schedule(state, task, task_id, attempt, reason, plugin_name, callback) do
    delay_ms = compute_delay(reason, attempt)
    timer_ref = Process.send_after(self(), {:fire_retry, task_id}, delay_ms)

    entry = %{
      task: Map.put(task, :retry_attempt, attempt),
      plugin_name: plugin_name,
      reason: reason,
      attempt: attempt,
      timer_ref: timer_ref,
      scheduled_at: System.monotonic_time(:millisecond),
      delay_ms: delay_ms,
      callback: callback
    }

    pending = Map.put(state.pending, task_id, entry)

    Logger.info(
      "Scheduled retry #{attempt}/#{@max_retries} for task #{task_id} in #{delay_ms}ms (#{inspect(reason)})"
    )

    {:reply, :ok, %{state | pending: pending, next_id: state.next_id + 1}}
  end

  defp handle_exhausted(task, task_id, plugin_name, manager, callback) do
    case find_fallback(task, plugin_name, manager) do
      {:ok, fallback_name} ->
        if callback, do: callback.({:retry, task, fallback_name})
        :ok

      :none ->
        if callback, do: callback.({:exhausted, task})
        Logger.warning("Task #{task_id} retries exhausted after #{@max_retries} attempts")
        {:error, :retries_exhausted}
    end
  end

  defp find_fallback(task, failed_plugin, manager) do
    capabilities = task[:capabilities] || [:coding]
    plugins = Manager.plugins_for_capabilities(capabilities, server: manager)

    fallback =
      Enum.find(plugins, fn p ->
        p.name != failed_plugin and p.status in [:healthy, :degraded]
      end)

    case fallback do
      nil -> :none
      plugin -> {:ok, plugin.name}
    end
  end

  defp compute_delay({:rate_limited, retry_after}, _attempt) when is_number(retry_after) do
    max(retry_after * 1_000, @base_delay_ms)
  end

  defp compute_delay(_reason, attempt) do
    delay = @base_delay_ms * Integer.pow(2, attempt - 1)
    min(delay, @max_delay_ms)
  end
end
