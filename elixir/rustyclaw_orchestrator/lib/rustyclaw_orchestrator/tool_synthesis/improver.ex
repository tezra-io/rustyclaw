defmodule RustyclawOrchestrator.ToolSynthesis.Improver do
  @moduledoc """
  GenServer for iterative tool improvement.

  When a synthesized tool fails, an agent can request a revision.
  The Improver loads the existing source, calls the LLM with context
  about the failure, compiles and tests the revised code, and swaps
  in the new version. Maintains a version history (max 5) with rollback.
  """

  use GenServer

  require Logger

  alias RustyclawOrchestrator.RustBridge
  alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Registry, Sandbox, StaticAnalyzer}

  @max_versions 5

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc """
  Request an improvement to an existing synthesized tool.

  ## Parameters

  - `tool_name` — the registered tool name
  - `opts` — keyword list:
    - `:failure_input` — the input that caused the failure
    - `:expected_output` — what the output should have been
    - `:error_message` — the error message from the failure
    - `:server` — GenServer name/pid (default: __MODULE__)
  """
  @spec improve(String.t(), keyword()) :: {:ok, map()} | {:error, term()}
  def improve(tool_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:improve, tool_name, opts}, 120_000)
  end

  @doc """
  Rollback a tool to its previous version.
  """
  @spec rollback(String.t(), keyword()) :: :ok | {:error, term()}
  def rollback(tool_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:rollback, tool_name})
  end

  @doc """
  List all stored versions for a tool.
  """
  @spec versions(String.t(), keyword()) :: {:ok, [map()]} | {:error, term()}
  def versions(tool_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:versions, tool_name})
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    bridge = Keyword.get(opts, :bridge)
    {:ok, %{bridge: bridge, histories: %{}}}
  end

  @impl true
  def handle_call({:improve, tool_name, opts}, _from, state) do
    case do_improve(tool_name, opts, state) do
      {:ok, result, state} -> {:reply, {:ok, result}, state}
      {:error, reason} -> {:reply, {:error, reason}, state}
    end
  end

  def handle_call({:rollback, tool_name}, _from, state) do
    case do_rollback(tool_name, state) do
      {:ok, state} -> {:reply, :ok, state}
      {:error, reason} -> {:reply, {:error, reason}, state}
    end
  end

  def handle_call({:versions, tool_name}, _from, state) do
    history = Map.get(state.histories, tool_name, [])

    versions =
      history
      |> Enum.with_index(1)
      |> Enum.map(fn {entry, idx} ->
        %{version: idx, created_at: entry.created_at, source_size: byte_size(entry.source)}
      end)

    {:reply, {:ok, versions}, state}
  end

  # --- Improvement Pipeline ---

  defp do_improve(tool_name, opts, state) do
    failure_input = Keyword.get(opts, :failure_input)
    expected_output = Keyword.get(opts, :expected_output)
    error_message = Keyword.get(opts, :error_message, "unknown error")

    with {:ok, current_source} <- load_current_source(tool_name, state),
         {:ok, revised_source} <-
           generate_revision(
             tool_name,
             current_source,
             failure_input,
             expected_output,
             error_message,
             opts,
             state
           ),
         :ok <- StaticAnalyzer.validate(revised_source),
         {:ok, new_module} <- compile_versioned(tool_name, revised_source, state),
         :ok <- validate_callbacks(new_module),
         :ok <- test_revised(new_module, failure_input, expected_output),
         :ok <- test_previous_inputs(new_module, tool_name, state) do
      state = swap_version(tool_name, current_source, revised_source, new_module, state)

      {:ok,
       %{
         name: tool_name,
         module: new_module,
         source: revised_source,
         version: length(Map.get(state.histories, tool_name, []))
       }, state}
    end
  end

  defp load_current_source(tool_name, state) do
    # Check in-memory history first
    case Map.get(state.histories, tool_name) do
      [latest | _] ->
        {:ok, latest.source}

      _ ->
        # Try loading from persistence
        dir = Persistence.storage_dir()
        source_path = Path.join(dir, "#{tool_name}.ex")

        case File.read(source_path) do
          {:ok, source} -> {:ok, source}
          {:error, _} -> {:error, :source_not_found}
        end
    end
  end

  defp generate_revision(
         tool_name,
         current_source,
         failure_input,
         expected_output,
         error_message,
         opts,
         state
       ) do
    prompt =
      build_improvement_prompt(
        tool_name,
        current_source,
        failure_input,
        expected_output,
        error_message
      )

    case call_bridge(tool_name, prompt, opts, state) do
      {:ok, response} -> extract_source_from_response(response)
      {:error, _} = error -> error
    end
  end

  defp build_improvement_prompt(
         tool_name,
         current_source,
         failure_input,
         expected_output,
         error_message
       ) do
    input_section = if failure_input, do: "\nFailure input: #{inspect(failure_input)}", else: ""

    output_section =
      if expected_output, do: "\nExpected output: #{inspect(expected_output)}", else: ""

    """
    Fix this synthesized Elixir tool. It failed during execution.

    Tool name: #{tool_name}
    Error: #{error_message}#{input_section}#{output_section}

    Current source code:
    ```elixir
    #{current_source}
    ```

    Requirements:
    - Keep the same module name and namespace (RustyclawOrchestrator.Synth.*)
    - Keep the same callbacks: name/0, description/0, parameters_schema/0, execute/1
    - Fix the bug that caused the error
    - Use ONLY allowed modules: Enum, Map, List, String, Regex, Jason, Integer, Float, Tuple, Keyword, MapSet, Stream, Range, Access, URI, Base, Bitwise, Date, Time, DateTime, NaiveDateTime
    - NO: import, use, require, File, System, Port, Process, Code, spawn, send, apply/3, defmacro

    Return ONLY the fixed module code, no explanations. Wrap in ```elixir code fences.
    """
  end

  defp call_bridge(name, prompt, opts, state) do
    if state.bridge do
      state.bridge.(name, prompt)
    else
      model = Keyword.get(opts, :model)
      bridge_opts = if model, do: [model: model], else: []
      RustBridge.run_task("tool_improver", prompt, bridge_opts)
    end
  end

  defp extract_source_from_response(%{"response" => text}), do: extract_source(text)
  defp extract_source_from_response(%{"text" => text}), do: extract_source(text)
  defp extract_source_from_response(text) when is_binary(text), do: extract_source(text)

  defp extract_source_from_response(response) when is_map(response) do
    text = Map.get(response, "output") || Map.get(response, "content") || ""
    extract_source(text)
  end

  defp extract_source(text) when is_binary(text) do
    extract_fenced(text) || extract_bare(text) || {:error, :no_source_in_response}
  end

  defp extract_source(_), do: {:error, :no_source_in_response}

  defp extract_fenced(text) do
    case Regex.run(~r/```elixir\s*\n(.*?)```/s, text) do
      [_, code] ->
        {:ok, String.trim(code)}

      nil ->
        case Regex.run(~r/```\s*\n(.*?)```/s, text) do
          [_, code] -> {:ok, String.trim(code)}
          nil -> nil
        end
    end
  end

  defp extract_bare(text) do
    trimmed = String.trim(text)
    if String.contains?(trimmed, "defmodule"), do: {:ok, trimmed}
  end

  defp compile_versioned(_tool_name, source, _state) do
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

  defp test_revised(_module, nil, _expected), do: :ok
  defp test_revised(_module, _input, nil), do: :ok

  defp test_revised(module, failure_input, expected_output) do
    case Sandbox.execute(module, failure_input, timeout: 10_000) do
      {:ok, ^expected_output} -> :ok
      {:ok, actual} -> {:error, {:revision_mismatch, expected: expected_output, actual: actual}}
      {:error, reason} -> {:error, {:revision_failed, reason}}
    end
  end

  defp test_previous_inputs(module, tool_name, state) do
    history = Map.get(state.histories, tool_name, [])

    test_inputs =
      history
      |> Enum.flat_map(fn entry -> entry.test_inputs end)
      |> Enum.uniq()

    regression_count = Enum.count(test_inputs, &regression?(module, &1))

    if regression_count == 0 do
      :ok
    else
      {:error, {:regression, regression_count}}
    end
  end

  defp regression?(module, {input, expected}) do
    case Sandbox.execute(module, input, timeout: 10_000) do
      {:ok, ^expected} -> false
      _ -> true
    end
  end

  defp swap_version(tool_name, old_source, new_source, new_module, state) do
    history = Map.get(state.histories, tool_name, [])

    # Build test input record from the current improvement
    new_entry = %{
      source: old_source,
      created_at: DateTime.utc_now(),
      test_inputs: []
    }

    history = [new_entry | history] |> Enum.take(@max_versions)
    state = put_in(state, [:histories, tool_name], history)

    # Update registry: unload old, register new
    Registry.unload(tool_name)
    Registry.register(tool_name, new_module, status: :probation)

    # Persist the versioned source
    persist_version(tool_name, new_source, length(history))

    state
  end

  defp persist_version(tool_name, source, version_num) do
    dir = Persistence.storage_dir()

    case File.mkdir_p(dir) do
      :ok ->
        version_path = Path.join(dir, "#{tool_name}_v#{version_num}.ex")
        File.write(version_path, source)

      {:error, reason} ->
        Logger.warning(
          "Failed to persist version #{version_num} for #{tool_name}: #{inspect(reason)}"
        )
    end
  end

  defp do_rollback(tool_name, state) do
    history = Map.get(state.histories, tool_name, [])

    case history do
      [] ->
        {:error, :no_previous_version}

      [previous | rest] ->
        # Recompile the previous version
        case compile_versioned(tool_name, previous.source, state) do
          {:ok, module} ->
            Registry.unload(tool_name)
            Registry.register(tool_name, module, status: :probation)
            state = put_in(state, [:histories, tool_name], rest)
            {:ok, state}

          {:error, reason} ->
            {:error, {:rollback_compile_failed, reason}}
        end
    end
  end
end
