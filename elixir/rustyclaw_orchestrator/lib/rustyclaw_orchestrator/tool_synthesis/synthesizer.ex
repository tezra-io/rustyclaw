defmodule RustyclawOrchestrator.ToolSynthesis.Synthesizer do
  @moduledoc """
  GenServer that orchestrates the full tool synthesis pipeline.

  Receives synthesis requests, calls the LLM via RustBridge to generate
  Elixir source code, then runs the compile → validate → test → register
  flow. Enforces rate limiting (max 3 synthesis attempts per agent per hour).

  On startup, loads persisted tools from disk before accepting requests.
  """

  use GenServer

  require Logger

  alias RustyclawOrchestrator.RustBridge
  alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Registry, Sandbox, StaticAnalyzer}

  @max_per_hour 3
  @hour_ms 3_600_000

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc """
  Synthesize a new tool from a capability description.

  ## Parameters

  - `request` — map with required keys:
    - `:capability` — description of what the tool should do
    - `:suggested_name` — snake_case name for the tool
  - `opts` — keyword list:
    - `:agent_id` — ID of the requesting agent (for rate limiting)
    - `:input_example` — example input map for testing
    - `:expected_output` — expected output string for testing
    - `:model` — LLM model override
    - `:server` — GenServer name/pid (default: __MODULE__)

  Returns `{:ok, tool_info}` or `{:error, reason}`.
  """
  @spec synthesize(map(), keyword()) :: {:ok, map()} | {:error, term()}
  def synthesize(request, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:synthesize, request, opts}, 120_000)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    bridge = Keyword.get(opts, :bridge)
    state = %{rate_limits: %{}, bridge: bridge}

    # Load persisted tools asynchronously so we don't block the supervisor
    {:ok, state, {:continue, :load_persisted}}
  end

  @impl true
  def handle_continue(:load_persisted, state) do
    case Persistence.load_all() do
      {:ok, count} when count > 0 ->
        Logger.info("Loaded #{count} persisted synthesized tool(s)")

      {:ok, 0} ->
        :ok
    end

    {:noreply, state}
  end

  @impl true
  def handle_call({:synthesize, request, opts}, _from, state) do
    agent_id = Keyword.get(opts, :agent_id, "unknown")

    case check_rate_limit(agent_id, state) do
      {:ok, state} ->
        result = do_synthesize(request, opts, state)
        state = record_attempt(agent_id, state)
        {:reply, result, state}

      {:error, _} = error ->
        {:reply, error, state}
    end
  end

  # --- Rate Limiting ---

  defp check_rate_limit(agent_id, state) do
    now = System.monotonic_time(:millisecond)
    attempts = Map.get(state.rate_limits, agent_id, [])
    recent = Enum.filter(attempts, &(now - &1 < @hour_ms))

    if length(recent) >= @max_per_hour do
      {:error, :rate_limited}
    else
      {:ok, %{state | rate_limits: Map.put(state.rate_limits, agent_id, recent)}}
    end
  end

  defp record_attempt(agent_id, state) do
    now = System.monotonic_time(:millisecond)
    attempts = Map.get(state.rate_limits, agent_id, [])
    %{state | rate_limits: Map.put(state.rate_limits, agent_id, [now | attempts])}
  end

  # --- Synthesis Pipeline ---

  defp do_synthesize(request, opts, state) do
    capability = Map.fetch!(request, :capability)
    suggested_name = Map.fetch!(request, :suggested_name)
    input_example = Keyword.get(opts, :input_example)
    expected_output = Keyword.get(opts, :expected_output)

    module_name = camelize(suggested_name)
    agent_id = Keyword.get(opts, :agent_id, "unknown")

    with {:ok, source} <-
           generate_code(
             capability,
             suggested_name,
             module_name,
             input_example,
             expected_output,
             opts,
             state
           ),
         :ok <- StaticAnalyzer.validate(source),
         {:ok, module} <- compile_source(source),
         :ok <- validate_callbacks(module),
         :ok <- test_with_example(module, input_example, expected_output),
         :ok <- register_tool(suggested_name, module, agent_id) do
      {:ok,
       %{
         name: suggested_name,
         module: module,
         source: source,
         status: :probation
       }}
    end
  end

  defp generate_code(capability, name, module_name, input_example, expected_output, opts, state) do
    prompt = build_prompt(capability, module_name, input_example, expected_output)

    case call_bridge(name, prompt, opts, state) do
      {:ok, response} -> extract_source_from_response(response)
      {:error, _} = error -> error
    end
  end

  defp call_bridge(name, prompt, opts, state) do
    if state.bridge do
      state.bridge.(name, prompt)
    else
      model = Keyword.get(opts, :model)
      bridge_opts = if model, do: [model: model], else: []
      RustBridge.run_task("tool_synthesizer", prompt, bridge_opts)
    end
  end

  defp extract_source_from_response(%{"response" => text}), do: extract_source(text)
  defp extract_source_from_response(%{"text" => text}), do: extract_source(text)
  defp extract_source_from_response(text) when is_binary(text), do: extract_source(text)

  defp extract_source_from_response(response) when is_map(response) do
    text = Map.get(response, "output") || Map.get(response, "content") || ""
    extract_source(text)
  end

  defp build_prompt(capability, module_name, input_example, expected_output) do
    example_section =
      if input_example && expected_output do
        """

        Input example: #{inspect(input_example)}
        Expected output: #{inspect(expected_output)}

        Your execute/1 function MUST produce the expected output for the given input.
        """
      else
        ""
      end

    """
    Generate an Elixir module that implements the SynthesizedTool behaviour.

    The module MUST:
    - Be named RustyclawOrchestrator.Synth.#{module_name}
    - Implement these callbacks:
      - name/0 returning a string
      - description/0 returning a string
      - parameters_schema/0 returning a map
      - execute/1 taking a map and returning {:ok, string} or {:error, string}
    - Use ONLY these modules: Enum, Map, List, String, Regex, Jason, Integer, Float, Tuple, Keyword, MapSet, Stream, Range, Access, URI, Base, Bitwise, Date, Time, DateTime, NaiveDateTime
    - NO: import, use, require, File, System, Port, Process, Code, spawn, send, apply/3, defmacro

    Capability needed: #{capability}
    #{example_section}
    Return ONLY the module code, no explanations. Wrap in ```elixir code fences.
    """
  end

  defp extract_source(text) when is_binary(text) do
    extract_fenced_elixir(text) ||
      extract_fenced_plain(text) ||
      extract_bare_defmodule(text) ||
      {:error, :no_source_in_response}
  end

  defp extract_source(_), do: {:error, :no_source_in_response}

  defp extract_fenced_elixir(text) do
    case Regex.run(~r/```elixir\s*\n(.*?)```/s, text) do
      [_, code] -> {:ok, String.trim(code)}
      nil -> nil
    end
  end

  defp extract_fenced_plain(text) do
    case Regex.run(~r/```\s*\n(.*?)```/s, text) do
      [_, code] -> {:ok, String.trim(code)}
      nil -> nil
    end
  end

  defp extract_bare_defmodule(text) do
    trimmed = String.trim(text)
    if String.contains?(trimmed, "defmodule"), do: {:ok, trimmed}
  end

  defp compile_source(source) do
    case Code.compile_string(source) do
      [{module, _bytecode}] -> {:ok, module}
      _ -> {:error, :compilation_failed}
    end
  rescue
    e -> {:error, {:compilation_error, Exception.message(e)}}
  end

  defp validate_callbacks(module) do
    required = [name: 0, description: 0, parameters_schema: 0, execute: 1]

    missing =
      Enum.reject(required, fn {fun, arity} ->
        function_exported?(module, fun, arity)
      end)

    if missing == [] do
      :ok
    else
      names = Enum.map(missing, fn {fun, arity} -> "#{fun}/#{arity}" end)
      {:error, {:missing_callbacks, names}}
    end
  end

  defp test_with_example(_module, nil, _expected), do: :ok
  defp test_with_example(_module, _input, nil), do: :ok

  defp test_with_example(module, input_example, expected_output) do
    case Sandbox.execute(module, input_example, timeout: 10_000) do
      {:ok, ^expected_output} ->
        :ok

      {:ok, actual} ->
        {:error, {:example_mismatch, expected: expected_output, actual: actual}}

      {:error, reason} ->
        {:error, {:example_failed, reason}}
    end
  end

  defp register_tool(name, module, agent_id) do
    Registry.register(name, module, author_agent: agent_id, status: :probation)
  end

  defp camelize(snake_name) do
    snake_name
    |> String.split("_")
    |> Enum.map_join(&String.capitalize/1)
  end
end
