defmodule RustyclawOrchestrator.AgentServer do
  @moduledoc """
  GenServer managing a single agent instance.

  Each agent runs as a supervised process with periodic health checks,
  message handling, task execution, parent-child relationships,
  accumulated state, state persistence, and memory limit enforcement.
  """

  use GenServer, restart: :transient

  alias RustyclawOrchestrator.{AgentDefinition, MessageProvenance, TraceStore}

  require Logger

  @health_check_interval 30_000
  @call_timeout 30_000
  @max_history 100
  @default_snapshot_dir "~/.rustyclaw/agent_snapshots"

  @type status :: :initializing | :idle | :running | :stopping
  @type health :: :healthy | :degraded | :unhealthy

  @type state :: %{
          definition: AgentDefinition.t(),
          session_id: String.t(),
          status: status(),
          health: health(),
          history: [map()],
          accumulated_state: map(),
          parent_pid: pid() | nil,
          child_pids: MapSet.t(),
          started_at: DateTime.t(),
          last_active_at: DateTime.t(),
          last_health_check: DateTime.t() | nil,
          recovery_attempts: non_neg_integer()
        }

  # --- Client API ---

  @doc """
  Start an agent server linked to the caller.
  `opts` must include `:definition` (AgentDefinition.t()).
  Optional: `:parent_pid` to establish parent-child relationship.
  """
  def start_link(opts) do
    definition = Keyword.fetch!(opts, :definition)
    name = via_registry(definition.name)
    GenServer.start_link(__MODULE__, opts, name: name)
  end

  @doc "Send a task to the agent for execution. Optionally pass provenance."
  def run_task(agent_name, task, provenance \\ nil) when is_binary(agent_name) do
    GenServer.call(via_registry(agent_name), {:run_task, task, provenance}, @call_timeout)
  end

  @doc "Send an async message to the agent. Optionally pass provenance."
  def send_message(agent_name, message, provenance \\ nil) when is_binary(agent_name) do
    GenServer.cast(via_registry(agent_name), {:send_message, message, provenance})
  end

  @doc "Get current agent state."
  def get_state(agent_name) when is_binary(agent_name) do
    GenServer.call(via_registry(agent_name), :get_state, @call_timeout)
  end

  @doc "Get agent health status."
  def get_health(agent_name) when is_binary(agent_name) do
    GenServer.call(via_registry(agent_name), :get_health, @call_timeout)
  end

  @doc "Delegate a task to a child agent, tracking the parent-child relationship."
  @spec delegate_to_child(String.t(), String.t(), String.t()) ::
          {:ok, term()} | {:error, term()}
  def delegate_to_child(parent_name, child_name, task)
      when is_binary(parent_name) and is_binary(child_name) do
    GenServer.call(
      via_registry(parent_name),
      {:delegate_to_child, child_name, task},
      @call_timeout
    )
  end

  @doc "Report a result back to the parent agent."
  @spec report_to_parent(String.t(), term()) :: :ok | {:error, :no_parent}
  def report_to_parent(child_name, result) when is_binary(child_name) do
    GenServer.call(via_registry(child_name), {:report_to_parent, result}, @call_timeout)
  end

  @doc "Update the agent's accumulated state with a map merge."
  @spec update_accumulated_state(String.t(), map()) :: :ok
  def update_accumulated_state(agent_name, updates)
      when is_binary(agent_name) and is_map(updates) do
    GenServer.call(via_registry(agent_name), {:update_accumulated_state, updates}, @call_timeout)
  end

  @doc "Get a snapshot of the agent's persistent state."
  @spec get_snapshot(String.t()) :: map()
  def get_snapshot(agent_name) when is_binary(agent_name) do
    GenServer.call(via_registry(agent_name), :get_snapshot, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    # Trap exits so terminate/2 is called on shutdown (needed for snapshot persistence)
    Process.flag(:trap_exit, true)

    definition = Keyword.fetch!(opts, :definition)
    parent_pid = Keyword.get(opts, :parent_pid)

    if parent_pid, do: Process.monitor(parent_pid)

    now = DateTime.utc_now()
    restored = maybe_restore_snapshot(definition.name)

    state = %{
      definition: definition,
      session_id: generate_session_id(),
      status: :idle,
      health: :healthy,
      history: Map.get(restored, :history, []),
      accumulated_state: Map.get(restored, :accumulated_state, %{}),
      parent_pid: parent_pid,
      child_pids: MapSet.new(),
      started_at: now,
      last_active_at: now,
      last_health_check: nil,
      recovery_attempts: 0
    }

    schedule_health_check()
    {:ok, state}
  end

  @impl true
  def handle_call({:run_task, task, provenance}, _from, %{health: :unhealthy} = state) do
    maybe_log_provenance(:task_rejected, state.definition.name, provenance)

    {:reply, {:error, :unhealthy},
     append_history(state, :task_rejected, %{task: task, reason: :unhealthy})}
  end

  def handle_call({:run_task, task, provenance}, _from, state) do
    case check_memory_limit(state) do
      :ok ->
        maybe_record_provenance(provenance)
        maybe_log_provenance(:task_executed, state.definition.name, provenance)

        state = %{state | status: :running, last_active_at: DateTime.utc_now()}

        # Task execution will be routed through RustBridge in TEZ-146.
        # For now, record the task and return a placeholder.
        result = {:ok, %{task: task, status: :pending_bridge}}

        state =
          state
          |> Map.put(:status, :idle)
          |> append_history(:task_executed, %{task: task, result: result})

        {:reply, result, state}

      {:error, _} = err ->
        state = append_history(state, :task_rejected, %{task: task, reason: :memory_limit})
        {:reply, err, state}
    end
  end

  def handle_call(:get_state, _from, state) do
    {:reply, externalize_state(state), state}
  end

  def handle_call(:get_health, _from, state) do
    {:reply, state.health, state}
  end

  def handle_call({:delegate_to_child, child_name, task}, from, state) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, child_name) do
      [{child_pid, _}] ->
        # Track child and monitor if not already tracked
        state =
          if MapSet.member?(state.child_pids, child_pid) do
            state
          else
            Process.monitor(child_pid)
            %{state | child_pids: MapSet.put(state.child_pids, child_pid)}
          end

        # Async delegation: spawn a task to avoid blocking this GenServer
        Task.start(fn ->
          result = run_task(child_name, task)
          GenServer.reply(from, result)
        end)

        state =
          state
          |> Map.put(:last_active_at, DateTime.utc_now())
          |> append_history(:delegated_to_child, %{child: child_name, task: task})

        {:noreply, state}

      [] ->
        {:reply, {:error, :child_not_found}, state}
    end
  end

  def handle_call({:report_to_parent, result}, _from, state) do
    case state.parent_pid do
      nil ->
        {:reply, {:error, :no_parent}, state}

      parent_pid when is_pid(parent_pid) ->
        send(parent_pid, {:child_report, state.definition.name, result})

        state =
          state
          |> Map.put(:last_active_at, DateTime.utc_now())
          |> append_history(:reported_to_parent, %{result: result})

        {:reply, :ok, state}
    end
  end

  def handle_call({:update_accumulated_state, updates}, _from, state) do
    new_acc = Map.merge(state.accumulated_state, updates)
    state = %{state | accumulated_state: new_acc, last_active_at: DateTime.utc_now()}

    case check_memory_limit(state) do
      :ok ->
        {:reply, :ok, state}

      {:error, _} = err ->
        Logger.warning("Agent #{state.definition.name} state update rejected: memory limit")
        {:reply, err, state}
    end
  end

  def handle_call(:get_snapshot, _from, state) do
    {:reply, build_snapshot(state), state}
  end

  @impl true
  def handle_cast({:send_message, message, provenance}, state) do
    maybe_record_provenance(provenance)
    maybe_log_provenance(:message_received, state.definition.name, provenance)

    state =
      state
      |> Map.put(:last_active_at, DateTime.utc_now())
      |> append_history(:message_received, %{message: message})

    {:noreply, state}
  end

  @impl true
  def handle_info(:health_check, state) do
    schedule_health_check()

    health = evaluate_health(state)
    state = %{state | health: health, last_health_check: DateTime.utc_now()}

    state =
      case health do
        :healthy ->
          %{state | recovery_attempts: 0}

        :degraded ->
          state

        :unhealthy ->
          %{state | recovery_attempts: state.recovery_attempts + 1}
      end

    {:noreply, state}
  end

  def handle_info({:child_report, child_name, result}, state) do
    state =
      state
      |> Map.put(:last_active_at, DateTime.utc_now())
      |> append_history(:child_reported, %{child: child_name, result: result})

    {:noreply, state}
  end

  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    state = %{state | child_pids: MapSet.delete(state.child_pids, pid)}

    state =
      if state.parent_pid == pid do
        %{state | parent_pid: nil}
      else
        state
      end

    {:noreply, state}
  end

  @impl true
  def terminate(_reason, state) do
    if state.definition.persistent do
      save_snapshot(state)
    end

    :ok
  end

  # --- Internals ---

  defp via_registry(agent_name) do
    {:via, Registry, {RustyclawOrchestrator.AgentRegistry, agent_name}}
  end

  defp schedule_health_check do
    Process.send_after(self(), :health_check, @health_check_interval)
  end

  defp generate_session_id do
    Base.hex_encode32(:crypto.strong_rand_bytes(8), case: :lower, padding: false)
  end

  defp evaluate_health(state) do
    cond do
      state.status == :stopping -> :unhealthy
      state.recovery_attempts >= 3 -> :unhealthy
      state.recovery_attempts >= 1 -> :degraded
      true -> :healthy
    end
  end

  defp append_history(state, event, data) do
    entry = Map.merge(data, %{event: event, timestamp: DateTime.utc_now()})
    history = Enum.take([entry | state.history], @max_history)
    %{state | history: history}
  end

  defp externalize_state(state) do
    %{
      definition: state.definition,
      session_id: state.session_id,
      status: state.status,
      health: state.health,
      history: state.history,
      accumulated_state: state.accumulated_state,
      parent_pid: state.parent_pid,
      child_pids: MapSet.to_list(state.child_pids),
      started_at: state.started_at,
      last_active_at: state.last_active_at,
      last_health_check: state.last_health_check,
      recovery_attempts: state.recovery_attempts
    }
  end

  # --- Memory limit enforcement ---

  defp check_memory_limit(state) do
    case state.definition.max_memory_mb do
      nil -> :ok
      limit_mb -> enforce_memory_limit(state.definition.name, limit_mb)
    end
  end

  defp enforce_memory_limit(agent_name, limit_mb) do
    case Process.info(self(), :memory) do
      {:memory, bytes} ->
        mb = bytes / (1024 * 1024)

        if mb > limit_mb do
          Logger.warning(
            "Agent #{agent_name} exceeded memory limit: #{Float.round(mb, 2)}MB > #{limit_mb}MB"
          )

          {:error, :memory_limit_exceeded}
        else
          :ok
        end

      nil ->
        :ok
    end
  end

  # --- State persistence (snapshots) ---

  defp snapshot_dir do
    Application.get_env(:rustyclaw_orchestrator, :snapshot_dir, @default_snapshot_dir)
    |> Path.expand()
  end

  defp snapshot_path(agent_name) do
    # Sanitize agent name to prevent path traversal
    safe_name = String.replace(agent_name, ~r"[^a-zA-Z0-9_\-]", "_")
    Path.join(snapshot_dir(), "#{safe_name}.snapshot.etf")
  end

  defp build_snapshot(state) do
    %{
      agent_name: state.definition.name,
      accumulated_state: state.accumulated_state,
      history: Enum.take(state.history, 20),
      last_active_at: state.last_active_at,
      snapshot_at: DateTime.utc_now()
    }
  end

  defp save_snapshot(state) do
    dir = snapshot_dir()
    File.mkdir_p!(dir)
    path = snapshot_path(state.definition.name)
    data = build_snapshot(state)

    case File.write(path, :erlang.term_to_binary(data)) do
      :ok ->
        Logger.debug("Saved snapshot for agent #{state.definition.name}")

      {:error, reason} ->
        Logger.warning("Failed to save snapshot for #{state.definition.name}: #{inspect(reason)}")
    end
  end

  defp maybe_restore_snapshot(agent_name) do
    path = snapshot_path(agent_name)

    case File.read(path) do
      {:ok, binary} ->
        try do
          data = :erlang.binary_to_term(binary, [:safe])
          Logger.debug("Restored snapshot for agent #{agent_name}")
          data
        rescue
          _ ->
            Logger.warning("Corrupt snapshot for #{agent_name}, starting fresh")
            %{}
        end

      {:error, _} ->
        %{}
    end
  end

  # --- Provenance helpers ---

  defp maybe_record_provenance(%MessageProvenance{} = prov) do
    TraceStore.record(prov)
  end

  defp maybe_record_provenance(_), do: :ok

  defp maybe_log_provenance(event, agent_name, %MessageProvenance{} = prov) do
    Logger.info("Agent #{event}",
      agent: agent_name,
      event: event,
      trace_id: prov.trace_id,
      origin_agent: prov.origin_agent,
      source_agent: prov.source_agent,
      kind: prov.kind,
      delegation_depth: prov.delegation_depth
    )
  end

  defp maybe_log_provenance(_event, _agent_name, _), do: :ok
end
