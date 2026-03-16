defmodule RustyclawOrchestrator.Plugins.TaskOrchestrator do
  @moduledoc """
  GenServer orchestrating dev sessions — iterates through issues,
  runs QualityGate after each, tracks progress via ProgressTracker,
  and handles failures via RetryScheduler.
  """

  use GenServer

  alias RustyclawOrchestrator.Plugins.{
    CronBridge,
    LinearIntegration,
    ProgressTracker,
    QualityGate,
    RetryScheduler
  }

  require Logger

  @call_timeout 30_000

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc """
  Start a new dev session.

  Config keys:
  - `:repo_path` — path to the git repository (required)
  - `:issues` — list of issue maps (required)
  - `:quality_gates` — list of gate configs (optional)
  - `:plugin_opts` — options passed to CronBridge (optional)
  """
  @spec start_session(map(), keyword()) :: {:ok, String.t()} | {:error, term()}
  def start_session(config, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:start_session, config}, @call_timeout)
  end

  @doc "Get the current status of a session."
  @spec get_status(String.t(), keyword()) :: {:ok, map()} | {:error, :not_found}
  def get_status(session_id, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:get_status, session_id}, @call_timeout)
  end

  @doc "Cancel an active session."
  @spec cancel_session(String.t(), keyword()) :: :ok | {:error, :not_found}
  def cancel_session(session_id, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:cancel_session, session_id}, @call_timeout)
  end

  @doc "List all sessions."
  @spec list_sessions(keyword()) :: [map()]
  def list_sessions(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :list_sessions, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    {:ok, %{sessions: %{}}}
  end

  @impl true
  def handle_call({:start_session, config}, _from, state) do
    session_id = "session-#{System.unique_integer([:positive])}"

    session = %{
      id: session_id,
      repo_path: config[:repo_path] || config["repo_path"],
      issues: config[:issues] || config["issues"] || [],
      quality_gates: config[:quality_gates] || config["quality_gates"] || [],
      plugin_opts: config[:plugin_opts] || config["plugin_opts"] || [],
      status: :running,
      current_index: 0,
      completed: [],
      failures: [],
      started_at: DateTime.utc_now(),
      finished_at: nil
    }

    sessions = Map.put(state.sessions, session_id, session)
    # Kick off async processing
    send(self(), {:process_next, session_id})
    {:reply, {:ok, session_id}, %{state | sessions: sessions}}
  end

  def handle_call({:get_status, session_id}, _from, state) do
    case Map.get(state.sessions, session_id) do
      nil -> {:reply, {:error, :not_found}, state}
      session -> {:reply, {:ok, session_summary(session)}, state}
    end
  end

  def handle_call({:cancel_session, session_id}, _from, state) do
    case Map.get(state.sessions, session_id) do
      nil ->
        {:reply, {:error, :not_found}, state}

      session ->
        updated = %{session | status: :cancelled, finished_at: DateTime.utc_now()}
        sessions = Map.put(state.sessions, session_id, updated)
        Logger.info("Session #{session_id} cancelled")
        {:reply, :ok, %{state | sessions: sessions}}
    end
  end

  def handle_call(:list_sessions, _from, state) do
    summaries = state.sessions |> Map.values() |> Enum.map(&session_summary/1)
    {:reply, summaries, state}
  end

  @impl true
  def handle_info({:process_next, session_id}, state) do
    case Map.get(state.sessions, session_id) do
      nil ->
        {:noreply, state}

      %{status: status} when status != :running ->
        {:noreply, state}

      session ->
        state = process_next_issue(session, state)
        {:noreply, state}
    end
  end

  def handle_info({:issue_result, session_id, identifier, result}, state) do
    case Map.get(state.sessions, session_id) do
      nil ->
        {:noreply, state}

      session ->
        session = record_result(session, identifier, result)
        sessions = Map.put(state.sessions, session_id, session)
        state = %{state | sessions: sessions}

        # Continue to next issue
        send(self(), {:process_next, session_id})
        {:noreply, state}
    end
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Internals ---

  defp process_next_issue(session, state) do
    issues = session.issues

    if session.current_index >= length(issues) do
      finished = %{session | status: :completed, finished_at: DateTime.utc_now()}

      Logger.info(
        "Session #{session.id} completed: #{length(session.completed)} done, #{length(session.failures)} failed"
      )

      sessions = Map.put(state.sessions, session.id, finished)
      %{state | sessions: sessions}
    else
      issue = Enum.at(issues, session.current_index)
      identifier = issue[:identifier] || issue["identifier"] || "unknown"
      session_id = session.id

      # Advance the index immediately
      updated = %{session | current_index: session.current_index + 1}
      sessions = Map.put(state.sessions, session.id, updated)

      # Record progress event
      ProgressTracker.record(session_id, {:session_issue_start, identifier})

      # Run async so the GenServer stays responsive
      orchestrator = self()

      spawn(fn ->
        result = run_issue(issue, updated)
        send(orchestrator, {:issue_result, session_id, identifier, result})
      end)

      %{state | sessions: sessions}
    end
  end

  defp run_issue(issue, session) do
    repo_path = session.repo_path
    issue_with_repo = Map.put(issue, :repo_path, repo_path)
    plugin_opts = session.plugin_opts

    case CronBridge.submit_coding_task(issue_with_repo, plugin_opts) do
      {:ok, result} ->
        run_quality_gates(result, session)

      {:error, reason} ->
        handle_failure(issue, reason, session)
    end
  end

  defp run_quality_gates(result, session) do
    case session.quality_gates do
      [] ->
        {:ok, result}

      gates ->
        case QualityGate.run(result, gates) do
          {:pass, _outputs} -> {:ok, result}
          {:fail, gate_name, output} -> {:error, {:quality_gate_failed, gate_name, output}}
        end
    end
  end

  defp handle_failure(issue, reason, _session) do
    identifier = issue[:identifier] || issue["identifier"] || "unknown"

    task = %{
      id: "retry-#{identifier}",
      description: "Retry #{identifier}",
      capabilities: [:coding],
      retry_attempt: 0
    }

    # Best-effort retry scheduling — don't fail the batch if scheduler is down
    try do
      RetryScheduler.schedule_retry(task, reason, "auto")
    catch
      _, _ -> :ok
    end

    {:error, reason}
  end

  defp record_result(session, identifier, result) do
    case result do
      {:ok, _} ->
        ProgressTracker.record(session.id, {:session_issue_complete, identifier})

        try do
          LinearIntegration.update_issue_state(identifier, :completed)
        catch
          _, _ -> :ok
        end

        %{session | completed: [{identifier, result} | session.completed]}

      {:error, _} ->
        ProgressTracker.record(session.id, {:session_issue_failed, identifier})

        try do
          LinearIntegration.update_issue_state(identifier, :failed)
        catch
          _, _ -> :ok
        end

        %{session | failures: [{identifier, result} | session.failures]}
    end
  end

  defp session_summary(session) do
    %{
      id: session.id,
      status: session.status,
      repo_path: session.repo_path,
      total_issues: length(session.issues),
      completed_count: length(session.completed),
      failure_count: length(session.failures),
      current_index: session.current_index,
      started_at: session.started_at,
      finished_at: session.finished_at
    }
  end
end
