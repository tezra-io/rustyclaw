defmodule RustyclawOrchestrator.Plugins.TaskQueue do
  @moduledoc """
  GenServer priority queue fed from Linear.

  Polls Linear for unstarted issues on a configurable team key,
  converts them to task maps, deduplicates against in-progress/completed
  work, and auto-assigns to available Workers via PluginManager.
  """

  use GenServer

  alias RustyclawOrchestrator.Plugins.{AutoRouter, CronBridge, LinearIntegration, Manager}

  require Logger

  @default_poll_interval_ms 5 * 60 * 1_000
  @call_timeout 30_000
  @ets_table :task_queue_tracking

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc "Pop the highest priority task from the queue."
  @spec pop_task(keyword()) :: {:ok, map()} | :empty
  def pop_task(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :pop_task, @call_timeout)
  end

  @doc "Manually push a task onto the queue."
  @spec push_task(map(), keyword()) :: :ok
  def push_task(task, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:push_task, task}, @call_timeout)
  end

  @doc "Get queue status: size, in-progress count, completed count."
  @spec status(keyword()) :: map()
  def status(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :status, @call_timeout)
  end

  @doc "List all tasks currently in the queue."
  @spec list_tasks(keyword()) :: [map()]
  def list_tasks(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :list_tasks, @call_timeout)
  end

  @doc "Remove a task from the queue by ID."
  @spec remove_task(String.t(), keyword()) :: :ok | {:error, :not_found}
  def remove_task(task_id, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:remove_task, task_id}, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    ets_table = Keyword.get(opts, :ets_table, @ets_table)
    init_ets(ets_table)

    state = %{
      queue: [],
      team_key: Keyword.get(opts, :team_key, "TEZ"),
      poll_interval_ms: Keyword.get(opts, :poll_interval_ms, @default_poll_interval_ms),
      linear_opts: Keyword.get(opts, :linear_opts, []),
      manager: Keyword.get(opts, :manager, Manager),
      auto_assign: Keyword.get(opts, :auto_assign, true),
      ets_table: ets_table,
      in_progress: MapSet.new(),
      completed: MapSet.new()
    }

    # Start polling if enabled (poll_interval > 0)
    if state.poll_interval_ms > 0 do
      schedule_poll(state.poll_interval_ms)
    end

    {:ok, state}
  end

  @impl true
  def handle_call(:pop_task, _from, state) do
    case state.queue do
      [] ->
        {:reply, :empty, state}

      [task | rest] ->
        identifier = task[:identifier] || task[:id]
        in_progress = MapSet.put(state.in_progress, identifier)
        :ets.insert(state.ets_table, {identifier, :in_progress, System.monotonic_time()})
        {:reply, {:ok, task}, %{state | queue: rest, in_progress: in_progress}}
    end
  end

  def handle_call({:push_task, task}, _from, state) do
    queue = insert_by_priority(state.queue, task)
    {:reply, :ok, %{state | queue: queue}}
  end

  def handle_call(:status, _from, state) do
    status = %{
      queue_size: length(state.queue),
      in_progress_count: MapSet.size(state.in_progress),
      completed_count: MapSet.size(state.completed)
    }

    {:reply, status, state}
  end

  def handle_call(:list_tasks, _from, state) do
    {:reply, state.queue, state}
  end

  def handle_call({:remove_task, task_id}, _from, state) do
    case Enum.split_with(state.queue, fn t -> (t[:id] || t[:identifier]) == task_id end) do
      {[], _} ->
        {:reply, {:error, :not_found}, state}

      {_removed, remaining} ->
        {:reply, :ok, %{state | queue: remaining}}
    end
  end

  @impl true
  def handle_info(:poll_linear, state) do
    state = do_poll(state)
    schedule_poll(state.poll_interval_ms)
    {:noreply, state}
  end

  def handle_info({:task_completed, identifier}, state) do
    in_progress = MapSet.delete(state.in_progress, identifier)
    completed = MapSet.put(state.completed, identifier)
    :ets.insert(state.ets_table, {identifier, :completed, System.monotonic_time()})
    {:noreply, %{state | in_progress: in_progress, completed: completed}}
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Internals ---

  defp init_ets(table_name) do
    if :ets.whereis(table_name) == :undefined do
      :ets.new(table_name, [:named_table, :set, :public])
    end
  end

  defp schedule_poll(interval_ms) do
    Process.send_after(self(), :poll_linear, interval_ms)
  end

  defp do_poll(state) do
    case LinearIntegration.fetch_todo_issues(state.team_key, state.linear_opts) do
      {:ok, issues} ->
        new_tasks = deduplicate_and_convert(issues, state)
        queue = merge_queues(state.queue, new_tasks)
        new_state = %{state | queue: queue}

        if state.auto_assign do
          auto_assign_tasks(new_state)
        else
          new_state
        end

      {:error, reason} ->
        Logger.warning("TaskQueue: Linear poll failed: #{inspect(reason)}")
        state
    end
  end

  defp deduplicate_and_convert(issues, state) do
    issues
    |> Enum.reject(fn issue ->
      id = issue[:identifier] || issue.identifier
      known?(id, state)
    end)
    |> Enum.map(&issue_to_task/1)
  end

  defp known?(identifier, state) do
    MapSet.member?(state.in_progress, identifier) or
      MapSet.member?(state.completed, identifier) or
      Enum.any?(state.queue, fn t -> (t[:identifier] || t[:id]) == identifier end)
  end

  defp issue_to_task(issue) do
    labels = issue[:labels] || []

    %{
      id: "queue-#{issue[:identifier]}-#{System.unique_integer([:positive])}",
      identifier: issue[:identifier],
      title: issue[:title] || "",
      description: issue[:description] || "",
      priority: issue[:priority] || 4,
      labels: labels,
      acceptance_criteria: extract_acceptance_criteria(issue[:description] || ""),
      capabilities: AutoRouter.route_task(%{labels: labels}),
      source: :task_queue,
      queued_at: DateTime.utc_now()
    }
  end

  defp extract_acceptance_criteria(description) do
    # Extract lines starting with "- [ ]" or "AC:" as acceptance criteria
    description
    |> String.split("\n")
    |> Enum.filter(fn line ->
      trimmed = String.trim(line)
      String.starts_with?(trimmed, "- [ ]") or String.starts_with?(trimmed, "AC:")
    end)
    |> Enum.join("\n")
  end

  defp insert_by_priority(queue, task) do
    priority = task[:priority] || 4

    {before, after_list} =
      Enum.split_while(queue, fn t -> (t[:priority] || 4) <= priority end)

    before ++ [task] ++ after_list
  end

  defp merge_queues(existing, new_tasks) do
    Enum.reduce(new_tasks, existing, &insert_by_priority(&2, &1))
  end

  defp auto_assign_tasks(state) do
    case state.queue do
      [] ->
        state

      [task | _rest] ->
        capabilities = task[:capabilities] || [:coding]

        case Manager.plugins_for_capabilities(capabilities, server: state.manager) do
          [_plugin | _] ->
            {:ok, popped_task} = pop_from_queue(state)
            spawn_auto_worker(popped_task, state)

            %{
              state
              | queue: tl(state.queue),
                in_progress: MapSet.put(state.in_progress, popped_task[:identifier])
            }

          [] ->
            state
        end
    end
  end

  defp pop_from_queue(%{queue: [task | _]}), do: {:ok, task}
  defp pop_from_queue(%{queue: []}), do: :empty

  defp spawn_auto_worker(task, state) do
    queue_pid = self()
    identifier = task[:identifier]

    opts =
      state.linear_opts ++
        [manager: state.manager, capabilities: task[:capabilities] || [:coding]]

    spawn(fn ->
      CronBridge.submit_coding_task(task, opts)
      send(queue_pid, {:task_completed, identifier})
    end)
  end
end
