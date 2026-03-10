defmodule RustyclawOrchestrator.TraceStore do
  @moduledoc """
  ETS-backed store for message provenance trace chains.

  Records provenance entries keyed by trace_id, enabling retrieval
  of full delegation chains for debugging and observability.
  """

  use GenServer

  alias RustyclawOrchestrator.MessageProvenance

  @table :rustyclaw_traces

  # --- Client API ---

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc "Record a provenance entry in the trace store."
  @spec record(MessageProvenance.t()) :: :ok
  def record(%MessageProvenance{} = provenance) do
    entry = {provenance.trace_id, provenance, provenance.timestamp}
    :ets.insert(@table, entry)
    :ok
  end

  @doc "Retrieve all provenance entries for a given trace_id, ordered by delegation_depth."
  @spec get_chain(String.t()) :: [MessageProvenance.t()]
  def get_chain(trace_id) do
    @table
    |> :ets.lookup(trace_id)
    |> Enum.map(fn {_id, provenance, _ts} -> provenance end)
    |> Enum.sort_by(& &1.delegation_depth)
  end

  @doc """
  Delete trace entries older than the given duration in seconds.

  Returns the number of entries removed.
  """
  @spec cleanup_older_than(non_neg_integer()) :: non_neg_integer()
  def cleanup_older_than(seconds) when is_integer(seconds) and seconds > 0 do
    cutoff = DateTime.add(DateTime.utc_now(), -seconds, :second)

    @table
    |> :ets.tab2list()
    |> Enum.reduce(0, fn {_trace_id, _prov, timestamp} = entry, count ->
      if DateTime.compare(timestamp, cutoff) == :lt do
        :ets.delete_object(@table, entry)
        count + 1
      else
        count
      end
    end)
  end

  @doc "Clear all trace entries."
  @spec clear() :: :ok
  def clear do
    :ets.delete_all_objects(@table)
    :ok
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :duplicate_bag, :public, read_concurrency: true])
    {:ok, %{}}
  end
end
