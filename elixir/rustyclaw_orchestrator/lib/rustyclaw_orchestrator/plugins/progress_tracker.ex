defmodule RustyclawOrchestrator.Plugins.ProgressTracker do
  @moduledoc """
  GenServer tracking worker progress, detecting loops and stuck workers.

  Receives events from Workers via `record/3`. Tracks per-worker state
  including recent artifacts (bounded queue), event timestamps, loop
  detection via Levenshtein similarity, and stuck detection via inactivity
  timeout.
  """

  use GenServer

  require Logger

  @default_window_size 5
  @default_similarity_threshold 0.85
  @default_stuck_timeout_ms 300_000
  @stuck_check_interval_ms 60_000

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc "Record an event from a worker."
  @spec record(GenServer.server(), term(), term()) :: :ok
  def record(server \\ __MODULE__, worker_id, event) do
    GenServer.cast(server, {:record, worker_id, event})
  end

  @doc "Get the tracking state for a specific worker."
  @spec get_worker_state(GenServer.server(), term()) :: {:ok, map()} | {:error, :not_found}
  def get_worker_state(server \\ __MODULE__, worker_id) do
    GenServer.call(server, {:get_worker_state, worker_id})
  end

  @doc "Remove tracking state for a worker."
  @spec clear_worker(GenServer.server(), term()) :: :ok
  def clear_worker(server \\ __MODULE__, worker_id) do
    GenServer.cast(server, {:clear_worker, worker_id})
  end

  @doc "List all tracked workers."
  @spec list_workers(GenServer.server()) :: [term()]
  def list_workers(server \\ __MODULE__) do
    GenServer.call(server, :list_workers)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    state = %{
      workers: %{},
      window_size: Keyword.get(opts, :window_size, @default_window_size),
      similarity_threshold:
        Keyword.get(opts, :similarity_threshold, @default_similarity_threshold),
      stuck_timeout_ms: Keyword.get(opts, :stuck_timeout_ms, @default_stuck_timeout_ms),
      on_loop_detected: Keyword.get(opts, :on_loop_detected),
      on_stuck_detected: Keyword.get(opts, :on_stuck_detected)
    }

    schedule_stuck_check(state.stuck_timeout_ms)
    {:ok, state}
  end

  @impl true
  def handle_cast({:record, worker_id, event}, state) do
    now = System.monotonic_time(:millisecond)
    worker_state = Map.get(state.workers, worker_id, new_worker_state())
    worker_state = %{worker_state | last_event_at: now, event_count: worker_state.event_count + 1}

    worker_state =
      case event do
        {:artifact, type, content} ->
          check_artifact(worker_state, type, content, state)

        _ ->
          worker_state
      end

    workers = Map.put(state.workers, worker_id, worker_state)
    {:noreply, %{state | workers: workers}}
  end

  def handle_cast({:clear_worker, worker_id}, state) do
    workers = Map.delete(state.workers, worker_id)
    {:noreply, %{state | workers: workers}}
  end

  @impl true
  def handle_call({:get_worker_state, worker_id}, _from, state) do
    case Map.get(state.workers, worker_id) do
      nil -> {:reply, {:error, :not_found}, state}
      ws -> {:reply, {:ok, ws}, state}
    end
  end

  def handle_call(:list_workers, _from, state) do
    {:reply, Map.keys(state.workers), state}
  end

  @impl true
  def handle_info(:check_stuck, state) do
    now = System.monotonic_time(:millisecond)
    check_stuck_workers(state.workers, now, state)
    schedule_stuck_check(state.stuck_timeout_ms)
    {:noreply, state}
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Private Helpers ---

  defp check_stuck_workers(workers, now, state) do
    Enum.each(workers, fn {worker_id, ws} ->
      maybe_notify_stuck(worker_id, ws, now, state)
    end)
  end

  defp maybe_notify_stuck(worker_id, ws, now, state) do
    stuck? = ws.last_event_at != nil and now - ws.last_event_at > state.stuck_timeout_ms

    if stuck? do
      Logger.warning(
        "Worker #{inspect(worker_id)} appears stuck (no events for #{state.stuck_timeout_ms}ms)"
      )

      if state.on_stuck_detected, do: state.on_stuck_detected.(worker_id)
    end
  end

  defp new_worker_state do
    %{
      recent_artifacts: :queue.new(),
      artifact_count: 0,
      last_event_at: nil,
      event_count: 0,
      consecutive_similar: 0,
      loop_detected: false
    }
  end

  defp check_artifact(worker_state, type, content, tracker_state) do
    artifact = %{type: type, content: content}

    {queue, _count} =
      bounded_enqueue(
        worker_state.recent_artifacts,
        artifact,
        worker_state.artifact_count,
        tracker_state.window_size
      )

    same_type_artifacts =
      queue
      |> :queue.to_list()
      |> Enum.filter(&(&1.type == type))
      |> Enum.map(& &1.content)

    consecutive_similar =
      if length(same_type_artifacts) >= 2 do
        recent = Enum.take(same_type_artifacts, -2)
        [prev, current] = recent

        similarity = levenshtein_similarity(prev, current)

        if similarity >= tracker_state.similarity_threshold do
          worker_state.consecutive_similar + 1
        else
          0
        end
      else
        0
      end

    loop_detected = consecutive_similar >= 3

    if loop_detected and not worker_state.loop_detected do
      Logger.warning(
        "Loop detected: #{consecutive_similar} consecutive similar artifacts of type #{inspect(type)}"
      )

      if tracker_state.on_loop_detected do
        tracker_state.on_loop_detected.(type, content)
      end
    end

    %{
      worker_state
      | recent_artifacts: queue,
        artifact_count: worker_state.artifact_count + 1,
        consecutive_similar: consecutive_similar,
        loop_detected: loop_detected
    }
  end

  defp bounded_enqueue(queue, item, count, max_size) do
    queue = :queue.in(item, queue)

    if count >= max_size do
      {_, queue} = :queue.out(queue)
      {queue, count}
    else
      {queue, count + 1}
    end
  end

  defp schedule_stuck_check(timeout_ms) do
    interval = min(timeout_ms, @stuck_check_interval_ms)
    Process.send_after(self(), :check_stuck, interval)
  end

  # --- Levenshtein Distance ---

  @doc false
  def levenshtein_similarity(a, b) when is_binary(a) and is_binary(b) do
    max_len = max(String.length(a), String.length(b))

    if max_len == 0 do
      1.0
    else
      distance = levenshtein_distance(a, b)
      1.0 - distance / max_len
    end
  end

  @doc false
  def levenshtein_distance(a, b) when is_binary(a) and is_binary(b) do
    a_chars = String.graphemes(a)
    b_chars = String.graphemes(b)
    a_len = length(a_chars)
    b_len = length(b_chars)

    # Optimize: if either is empty, distance is the other's length
    cond do
      a_len == 0 ->
        b_len

      b_len == 0 ->
        a_len

      true ->
        initial_row = Enum.to_list(0..b_len)

        a_chars
        |> Enum.with_index(1)
        |> Enum.reduce(initial_row, fn {a_char, i}, prev_row ->
          compute_row(a_char, i, b_chars, prev_row)
        end)
        |> List.last()
    end
  end

  defp compute_row(a_char, i, b_chars, prev_row) do
    b_chars
    |> Enum.with_index(1)
    |> Enum.reduce({[i], hd(prev_row)}, fn {b_char, j}, {curr_row, diag} ->
      cost = if a_char == b_char, do: 0, else: 1
      prev_above = Enum.at(prev_row, j)
      prev_left = hd(curr_row)
      val = min(min(prev_left + 1, prev_above + 1), diag + cost)
      {[val | curr_row], prev_above}
    end)
    |> then(fn {curr_row, _} -> Enum.reverse(curr_row) end)
  end
end
