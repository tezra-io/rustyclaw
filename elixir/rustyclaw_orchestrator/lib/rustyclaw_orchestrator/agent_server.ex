defmodule RustyclawOrchestrator.AgentServer do
  @moduledoc """
  GenServer managing a single agent instance.

  Each agent runs as a supervised process with periodic health checks,
  message handling, and task execution capabilities.
  """

  use GenServer, restart: :transient

  alias RustyclawOrchestrator.{AgentDefinition, MessageProvenance, TraceStore}

  require Logger

  @health_check_interval 30_000
  @call_timeout 30_000

  @type status :: :initializing | :idle | :running | :stopping
  @type health :: :healthy | :degraded | :unhealthy

  @type state :: %{
          definition: AgentDefinition.t(),
          session_id: String.t(),
          status: status(),
          health: health(),
          history: [map()],
          started_at: DateTime.t(),
          last_health_check: DateTime.t() | nil,
          recovery_attempts: non_neg_integer()
        }

  # --- Client API ---

  @doc """
  Start an agent server linked to the caller.
  `opts` must include `:definition` (AgentDefinition.t()).
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

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    definition = Keyword.fetch!(opts, :definition)

    state = %{
      definition: definition,
      session_id: generate_session_id(),
      status: :idle,
      health: :healthy,
      history: [],
      started_at: DateTime.utc_now(),
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
    maybe_record_provenance(provenance)
    maybe_log_provenance(:task_executed, state.definition.name, provenance)

    state = %{state | status: :running}

    # Task execution will be routed through RustBridge in TEZ-146.
    # For now, record the task and return a placeholder.
    result = {:ok, %{task: task, status: :pending_bridge}}

    state =
      state
      |> Map.put(:status, :idle)
      |> append_history(:task_executed, %{task: task, result: result})

    {:reply, result, state}
  end

  def handle_call(:get_state, _from, state) do
    {:reply, state, state}
  end

  def handle_call(:get_health, _from, state) do
    {:reply, state.health, state}
  end

  @impl true
  def handle_cast({:send_message, message, provenance}, state) do
    maybe_record_provenance(provenance)
    maybe_log_provenance(:message_received, state.definition.name, provenance)

    state = append_history(state, :message_received, %{message: message})
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
    # Keep last 100 history entries
    history = Enum.take([entry | state.history], 100)
    %{state | history: history}
  end

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
