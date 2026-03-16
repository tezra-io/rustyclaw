defmodule RustyclawOrchestrator.ToolSynthesis.Registry do
  @moduledoc """
  ETS-backed registry for synthesized tools.

  Stores tool name → module + metadata mappings.
  Tracks invocation count, success rate, and average latency.
  """

  @table :rustyclaw_synth_tools

  @type status :: :probation | :promoted | :suspended | :deprecated

  @type entry :: %{
          name: String.t(),
          module: module(),
          author_agent: String.t() | nil,
          created_at: DateTime.t(),
          status: status(),
          invocation_count: non_neg_integer(),
          success_count: non_neg_integer(),
          total_latency_ms: non_neg_integer(),
          description: String.t(),
          parameters_schema: map()
        }

  @doc "Initialize the ETS table. Called once at application start."
  @spec init() :: :ok
  def init do
    :ets.new(@table, [:named_table, :set, :public, read_concurrency: true])
    :ok
  end

  @doc """
  Register a synthesized tool.

  Options:
  - `:author_agent` — name of the agent that created this tool
  - `:status` — initial status (default: `:probation`)
  """
  @spec register(String.t(), module(), keyword()) :: :ok | {:error, :already_exists}
  def register(name, module, opts \\ []) when is_binary(name) and is_atom(module) do
    case :ets.lookup(@table, name) do
      [{^name, _}] ->
        {:error, :already_exists}

      [] ->
        entry = %{
          name: name,
          module: module,
          author_agent: Keyword.get(opts, :author_agent),
          created_at: DateTime.utc_now(),
          status: Keyword.get(opts, :status, :probation),
          invocation_count: 0,
          success_count: 0,
          total_latency_ms: 0,
          description: safe_call(module, :description, ""),
          parameters_schema: safe_call(module, :parameters_schema, %{})
        }

        :ets.insert(@table, {name, entry})
        :ok
    end
  end

  @doc "Look up a tool by name."
  @spec lookup(String.t()) :: {:ok, entry()} | {:error, :not_found}
  def lookup(name) when is_binary(name) do
    case :ets.lookup(@table, name) do
      [{^name, entry}] -> {:ok, entry}
      [] -> {:error, :not_found}
    end
  end

  @doc """
  Update metrics after a tool invocation.

  `success` is a boolean, `latency_ms` is the execution time.
  """
  @spec update_metrics(String.t(), boolean(), non_neg_integer()) ::
          :ok | {:error, :not_found}
  def update_metrics(name, success, latency_ms)
      when is_binary(name) and is_boolean(success) and is_integer(latency_ms) do
    case :ets.lookup(@table, name) do
      [{^name, entry}] ->
        entry = %{
          entry
          | invocation_count: entry.invocation_count + 1,
            success_count: entry.success_count + if(success, do: 1, else: 0),
            total_latency_ms: entry.total_latency_ms + latency_ms
        }

        :ets.insert(@table, {name, entry})
        :ok

      [] ->
        {:error, :not_found}
    end
  end

  @doc "Update the status of a tool."
  @spec update_status(String.t(), status()) :: :ok | {:error, :not_found}
  def update_status(name, status)
      when is_binary(name) and status in [:probation, :promoted, :suspended, :deprecated] do
    case :ets.lookup(@table, name) do
      [{^name, entry}] ->
        :ets.insert(@table, {name, %{entry | status: status}})
        :ok

      [] ->
        {:error, :not_found}
    end
  end

  @doc "Unload (delete) a tool from the registry."
  @spec unload(String.t()) :: :ok
  def unload(name) when is_binary(name) do
    :ets.delete(@table, name)
    :ok
  end

  @doc "List all registered tools. Optionally filter by status."
  @spec list(keyword()) :: [entry()]
  def list(opts \\ []) do
    status_filter = Keyword.get(opts, :status)

    @table
    |> :ets.tab2list()
    |> Enum.map(fn {_name, entry} -> entry end)
    |> then(fn entries ->
      if status_filter do
        Enum.filter(entries, &(&1.status == status_filter))
      else
        entries
      end
    end)
  end

  @doc "Return the success rate for a tool (0.0–1.0), or nil if no invocations."
  @spec success_rate(String.t()) :: float() | nil | {:error, :not_found}
  def success_rate(name) when is_binary(name) do
    case lookup(name) do
      {:ok, %{invocation_count: 0}} -> nil
      {:ok, entry} -> entry.success_count / entry.invocation_count
      error -> error
    end
  end

  @doc "Return the average latency in ms, or nil if no invocations."
  @spec avg_latency(String.t()) :: float() | nil | {:error, :not_found}
  def avg_latency(name) when is_binary(name) do
    case lookup(name) do
      {:ok, %{invocation_count: 0}} -> nil
      {:ok, entry} -> entry.total_latency_ms / entry.invocation_count
      error -> error
    end
  end

  @doc "Clear all entries. Used in tests."
  @spec clear() :: :ok
  def clear do
    :ets.delete_all_objects(@table)
    :ok
  end

  defp safe_call(module, fun, default) do
    if function_exported?(module, fun, 0) do
      apply(module, fun, [])
    else
      default
    end
  end
end
