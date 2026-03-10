defmodule RustyclawOrchestrator.MessageProvenance do
  @moduledoc """
  Tracks the origin and propagation path of inter-agent messages.

  Every message flowing through the orchestration layer carries provenance
  metadata: where it originated, how many delegation hops it has traversed,
  and which trace chain it belongs to.
  """

  @type kind :: :external_user | :inter_agent | :internal_system | :delegation

  @type t :: %__MODULE__{
          kind: kind(),
          trace_id: String.t(),
          origin_agent: String.t(),
          source_agent: String.t(),
          source_channel: String.t() | nil,
          delegation_depth: non_neg_integer(),
          timestamp: DateTime.t(),
          parent_trace_id: String.t() | nil
        }

  @enforce_keys [:kind, :trace_id, :origin_agent, :source_agent, :timestamp]
  defstruct [
    :kind,
    :trace_id,
    :origin_agent,
    :source_agent,
    :source_channel,
    :parent_trace_id,
    :timestamp,
    delegation_depth: 0
  ]

  @doc """
  Create a new provenance entry.

  `kind` is one of `:external_user | :inter_agent | :internal_system | :delegation`.

  `opts` supports:
  - `:origin_agent` (required) — agent that started the chain
  - `:source_agent` — immediate sender (defaults to origin_agent)
  - `:source_channel` — external channel name, if applicable
  - `:parent_trace_id` — trace ID of a parent delegation chain
  """
  @spec new(kind(), keyword()) :: t()
  def new(kind, opts) do
    origin = Keyword.fetch!(opts, :origin_agent)

    %__MODULE__{
      kind: kind,
      trace_id: generate_trace_id(),
      origin_agent: origin,
      source_agent: Keyword.get(opts, :source_agent, origin),
      source_channel: Keyword.get(opts, :source_channel),
      parent_trace_id: Keyword.get(opts, :parent_trace_id),
      delegation_depth: 0,
      timestamp: DateTime.utc_now()
    }
  end

  @doc """
  Create child provenance for a delegation hop.

  Preserves the trace_id, increments delegation_depth, and updates source_agent
  to the agent performing the delegation.
  """
  @spec propagate(t(), keyword()) :: t()
  def propagate(%__MODULE__{} = parent, opts) do
    %__MODULE__{
      kind: :delegation,
      trace_id: parent.trace_id,
      origin_agent: parent.origin_agent,
      source_agent: Keyword.fetch!(opts, :source_agent),
      source_channel: parent.source_channel,
      parent_trace_id: parent.parent_trace_id,
      delegation_depth: parent.delegation_depth + 1,
      timestamp: DateTime.utc_now()
    }
  end

  @doc "Serialize provenance to a plain map (for JSON / RustBridge payloads)."
  @spec to_map(t()) :: map()
  def to_map(%__MODULE__{} = p) do
    %{
      "kind" => Atom.to_string(p.kind),
      "trace_id" => p.trace_id,
      "origin_agent" => p.origin_agent,
      "source_agent" => p.source_agent,
      "source_channel" => p.source_channel,
      "delegation_depth" => p.delegation_depth,
      "timestamp" => DateTime.to_iso8601(p.timestamp),
      "parent_trace_id" => p.parent_trace_id
    }
  end

  @doc "Deserialize provenance from a plain map."
  @spec from_map(map()) :: {:ok, t()} | {:error, term()}
  def from_map(map) when is_map(map) do
    kind = parse_kind(map["kind"])

    case kind do
      {:ok, kind_atom} ->
        {:ok,
         %__MODULE__{
           kind: kind_atom,
           trace_id: map["trace_id"],
           origin_agent: map["origin_agent"],
           source_agent: map["source_agent"],
           source_channel: map["source_channel"],
           delegation_depth: map["delegation_depth"] || 0,
           timestamp: parse_timestamp(map["timestamp"]),
           parent_trace_id: map["parent_trace_id"]
         }}

      {:error, _} = err ->
        err
    end
  end

  # --- Internals ---

  defp generate_trace_id do
    Base.hex_encode32(:crypto.strong_rand_bytes(10), case: :lower, padding: false)
  end

  @valid_kinds ~w(external_user inter_agent internal_system delegation)

  defp parse_kind(kind) when kind in @valid_kinds do
    {:ok, String.to_existing_atom(kind)}
  end

  defp parse_kind(kind)
       when is_atom(kind) and
              kind in [:external_user, :inter_agent, :internal_system, :delegation] do
    {:ok, kind}
  end

  defp parse_kind(other), do: {:error, {:invalid_kind, other}}

  defp parse_timestamp(nil), do: DateTime.utc_now()

  defp parse_timestamp(ts) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _offset} -> dt
      {:error, _} -> DateTime.utc_now()
    end
  end

  defp parse_timestamp(%DateTime{} = dt), do: dt
end
