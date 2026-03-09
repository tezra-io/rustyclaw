defmodule RustyclawOrchestrator.SubAgentSession do
  @moduledoc """
  Tracks sub-agent task sessions in ETS.

  Sessions follow a lifecycle: pending → active → completed/failed/cancelled.
  All sessions are stored in an ETS table for fast in-memory access.
  """

  @type status :: :pending | :active | :completed | :failed | :cancelled

  @type t :: %__MODULE__{
          id: String.t(),
          agent_name: String.t(),
          parent_agent: String.t() | nil,
          task: String.t(),
          status: status(),
          result: term(),
          started_at: DateTime.t(),
          completed_at: DateTime.t() | nil,
          metadata: map()
        }

  defstruct [
    :id,
    :agent_name,
    :parent_agent,
    :task,
    :started_at,
    :completed_at,
    status: :pending,
    result: nil,
    metadata: %{}
  ]

  @table :rustyclaw_sessions

  @doc "Initialize the ETS table. Called once at application start."
  @spec init() :: :ok
  def init do
    :ets.new(@table, [:named_table, :set, :public, read_concurrency: true])
    :ok
  end

  @doc "Create a new session."
  @spec create(String.t(), String.t(), keyword()) :: t()
  def create(agent_name, task, opts \\ []) do
    session = %__MODULE__{
      id: generate_id(),
      agent_name: agent_name,
      parent_agent: Keyword.get(opts, :parent_agent),
      task: task,
      status: :pending,
      started_at: DateTime.utc_now(),
      metadata: Keyword.get(opts, :metadata, %{})
    }

    :ets.insert(@table, {session.id, session})
    session
  end

  @doc "Activate a pending session."
  @spec activate(String.t()) :: {:ok, t()} | {:error, :not_found | :invalid_transition}
  def activate(session_id) do
    transition(session_id, :active, [:pending])
  end

  @doc "Mark a session as completed with a result."
  @spec complete(String.t(), term()) :: {:ok, t()} | {:error, :not_found | :invalid_transition}
  def complete(session_id, result \\ nil) do
    with {:ok, session} <- transition(session_id, :completed, [:pending, :active]) do
      session = %{session | result: result, completed_at: DateTime.utc_now()}
      :ets.insert(@table, {session.id, session})
      {:ok, session}
    end
  end

  @doc "Mark a session as failed with a reason."
  @spec fail(String.t(), term()) :: {:ok, t()} | {:error, :not_found | :invalid_transition}
  def fail(session_id, reason \\ nil) do
    with {:ok, session} <- transition(session_id, :failed, [:pending, :active]) do
      session = %{session | result: reason, completed_at: DateTime.utc_now()}
      :ets.insert(@table, {session.id, session})
      {:ok, session}
    end
  end

  @doc "Cancel a session."
  @spec cancel(String.t()) :: {:ok, t()} | {:error, :not_found | :invalid_transition}
  def cancel(session_id) do
    transition(session_id, :cancelled, [:pending, :active])
  end

  @doc "Get a session by ID."
  @spec get(String.t()) :: {:ok, t()} | {:error, :not_found}
  def get(session_id) do
    case :ets.lookup(@table, session_id) do
      [{^session_id, session}] -> {:ok, session}
      [] -> {:error, :not_found}
    end
  end

  @doc "List all sessions, optionally filtered by agent name or status."
  @spec list(keyword()) :: [t()]
  def list(opts \\ []) do
    agent_name = Keyword.get(opts, :agent_name)
    status = Keyword.get(opts, :status)

    @table
    |> :ets.tab2list()
    |> Enum.map(fn {_id, session} -> session end)
    |> maybe_filter(:agent_name, agent_name)
    |> maybe_filter(:status, status)
  end

  @doc "Delete a session by ID."
  @spec delete(String.t()) :: :ok
  def delete(session_id) do
    :ets.delete(@table, session_id)
    :ok
  end

  @doc "Clear all sessions."
  @spec clear() :: :ok
  def clear do
    :ets.delete_all_objects(@table)
    :ok
  end

  @doc "Count sessions, optionally filtered by status."
  @spec count(keyword()) :: non_neg_integer()
  def count(opts \\ []) do
    list(opts) |> length()
  end

  # --- Internals ---

  defp transition(session_id, new_status, valid_from) do
    case get(session_id) do
      {:ok, session} ->
        if session.status in valid_from do
          session = %{session | status: new_status}
          :ets.insert(@table, {session.id, session})
          {:ok, session}
        else
          {:error, :invalid_transition}
        end

      {:error, :not_found} ->
        {:error, :not_found}
    end
  end

  defp maybe_filter(sessions, _field, nil), do: sessions

  defp maybe_filter(sessions, field, value),
    do: Enum.filter(sessions, &(Map.get(&1, field) == value))

  defp generate_id do
    Base.hex_encode32(:crypto.strong_rand_bytes(10), case: :lower, padding: false)
  end
end
