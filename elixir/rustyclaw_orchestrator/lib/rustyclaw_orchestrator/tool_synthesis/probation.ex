defmodule RustyclawOrchestrator.ToolSynthesis.Probation do
  @moduledoc """
  Lifecycle state machine for synthesized tools.

  Manages tool transitions through probation states:

      :probation → :promoted (success rate ≥ 80% after 10+ runs, auto_promote=true)
      :probation → :deprecated (success rate < 50% after 10+ runs)
      :probation → :suspended (crash/timeout/blocked output)
      :promoted  → :suspended (sliding window failure spike > 50%)

  After each tool invocation, `record_invocation/3` evaluates thresholds
  and triggers auto-transitions. On promotion, the tool is persisted.
  On deprecation, the tool is unloaded from the registry.
  """

  use GenServer

  require Logger

  alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Registry}

  @default_config %{
    probation_invocations: 10,
    min_success_rate: 0.8,
    deprecation_threshold: 0.5,
    auto_promote: false,
    sliding_window_size: 10
  }

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc """
  Record the result of a tool invocation and evaluate lifecycle transitions.

  - `tool_name` — the registered tool name
  - `success` — whether the invocation succeeded
  - `opts` — keyword options:
    - `:latency_ms` — execution time in milliseconds
    - `:crash` — true if the tool crashed/timed out/produced blocked output
    - `:source` — source code string (needed for persistence on promotion)
    - `:server` — GenServer name/pid (default: __MODULE__)
  """
  @spec record_invocation(String.t(), boolean(), keyword()) :: :ok | {:transition, atom()}
  def record_invocation(tool_name, success, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:record_invocation, tool_name, success, opts})
  end

  @doc """
  Get the current probation state for a tool.
  """
  @spec get_state(String.t(), keyword()) :: {:ok, map()} | {:error, :not_tracked}
  def get_state(tool_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:get_state, tool_name})
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    config =
      @default_config
      |> Map.merge(Map.new(Keyword.get(opts, :config, [])))

    {:ok, %{config: config, windows: %{}, sources: %{}}}
  end

  @impl true
  def handle_call({:record_invocation, tool_name, success, opts}, _from, state) do
    crash = Keyword.get(opts, :crash, false)
    source = Keyword.get(opts, :source)

    state =
      if source do
        put_in(state, [:sources, tool_name], source)
      else
        state
      end

    case Registry.lookup(tool_name) do
      {:ok, entry} ->
        {result, state} = evaluate(tool_name, entry, success, crash, state)
        {:reply, result, state}

      {:error, :not_found} ->
        {:reply, :ok, state}
    end
  end

  def handle_call({:get_state, tool_name}, _from, state) do
    case Registry.lookup(tool_name) do
      {:ok, entry} ->
        window = Map.get(state.windows, tool_name, [])

        info = %{
          status: entry.status,
          invocation_count: entry.invocation_count,
          success_rate: safe_success_rate(entry),
          window: window,
          window_failure_rate: window_failure_rate(window)
        }

        {:reply, {:ok, info}, state}

      {:error, :not_found} ->
        {:reply, {:error, :not_tracked}, state}
    end
  end

  # --- Evaluation Logic ---

  defp evaluate(tool_name, entry, success, crash, state) do
    cond do
      crash and entry.status in [:probation, :promoted] ->
        do_suspend(tool_name, state)

      entry.status == :probation ->
        evaluate_probation(tool_name, entry, state)

      entry.status == :promoted ->
        evaluate_promoted(tool_name, success, state)

      true ->
        {:ok, state}
    end
  end

  defp evaluate_probation(tool_name, entry, state) do
    config = state.config

    if entry.invocation_count >= config.probation_invocations do
      rate = safe_success_rate(entry)

      cond do
        rate < config.deprecation_threshold ->
          do_deprecate(tool_name, state)

        rate >= config.min_success_rate and config.auto_promote ->
          do_promote(tool_name, state)

        true ->
          {:ok, state}
      end
    else
      {:ok, state}
    end
  end

  defp evaluate_promoted(tool_name, success, state) do
    window_size = state.config.sliding_window_size
    window = Map.get(state.windows, tool_name, [])
    window = Enum.take([success | window], window_size)
    state = put_in(state, [:windows, tool_name], window)

    if length(window) >= window_size and window_failure_rate(window) > 0.5 do
      do_suspend(tool_name, state)
    else
      {:ok, state}
    end
  end

  # --- Transitions ---

  defp do_promote(tool_name, state) do
    Logger.info("Tool #{tool_name} promoted — passed probation thresholds")
    Registry.update_status(tool_name, :promoted)

    source = Map.get(state.sources, tool_name)

    if source do
      case Registry.lookup(tool_name) do
        {:ok, entry} ->
          metadata = %{
            author_agent: entry.author_agent,
            status: "promoted",
            promoted_at: DateTime.to_iso8601(DateTime.utc_now()),
            invocation_count: entry.invocation_count,
            success_rate: safe_success_rate(entry)
          }

          Persistence.save(tool_name, source, metadata)

        {:error, :not_found} ->
          :ok
      end
    end

    {{:transition, :promoted}, state}
  end

  defp do_deprecate(tool_name, state) do
    Logger.info("Tool #{tool_name} deprecated — success rate below threshold")
    Registry.update_status(tool_name, :deprecated)
    Registry.unload(tool_name)
    state = update_in(state, [:windows], &Map.delete(&1, tool_name))
    state = update_in(state, [:sources], &Map.delete(&1, tool_name))
    {{:transition, :deprecated}, state}
  end

  defp do_suspend(tool_name, state) do
    Logger.info("Tool #{tool_name} suspended — crash or failure spike detected")
    Registry.update_status(tool_name, :suspended)
    state = update_in(state, [:windows], &Map.delete(&1, tool_name))
    {{:transition, :suspended}, state}
  end

  # --- Helpers ---

  defp safe_success_rate(%{invocation_count: 0}), do: 0.0

  defp safe_success_rate(%{success_count: sc, invocation_count: ic}) do
    sc / ic
  end

  defp window_failure_rate([]), do: 0.0

  defp window_failure_rate(window) do
    failures = Enum.count(window, &(not &1))
    failures / length(window)
  end
end
