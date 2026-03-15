defmodule RustyclawOrchestrator.ToolSynthesis.Persistence do
  @moduledoc """
  File-backed storage for promoted synthesized tools.

  Saves tool source code and metadata to disk so they survive restarts.
  On startup, persisted tools are re-validated, compiled, and registered.

  Storage format per tool:
  - `<name>.ex`        — Elixir source code
  - `<name>.meta.json` — metadata (author, status, static analysis results)
  """

  require Logger

  alias RustyclawOrchestrator.ToolSynthesis.{Registry, StaticAnalyzer}

  @default_dir Path.expand("~/.rustyclaw/synthesized_tools")

  @doc "Return the configured storage directory."
  @spec storage_dir() :: String.t()
  def storage_dir do
    Application.get_env(:rustyclaw_orchestrator, :synthesized_tools_dir, @default_dir)
  end

  @doc """
  Save a promoted tool's source and metadata to disk.

  Creates the storage directory if it doesn't exist.
  """
  @spec save(String.t(), String.t(), map()) :: :ok | {:error, term()}
  def save(name, source, metadata) when is_binary(name) and is_binary(source) do
    dir = storage_dir()
    json = Jason.encode!(metadata, pretty: true)

    with :ok <- File.mkdir_p(dir),
         :ok <- File.write(source_path(dir, name), source) do
      File.write(meta_path(dir, name), json)
    end
  end

  @doc """
  Delete a persisted tool from disk.
  """
  @spec delete(String.t()) :: :ok
  def delete(name) when is_binary(name) do
    dir = storage_dir()
    File.rm(source_path(dir, name))
    File.rm(meta_path(dir, name))
    :ok
  end

  @doc """
  Load all persisted tools from disk.

  For each tool found:
  1. Read source and metadata
  2. Re-run static analysis
  3. Compile the module
  4. Register in the SynthesizedToolRegistry

  Tools that fail any step are skipped with a warning log.
  Returns the count of successfully loaded tools.
  """
  @spec load_all() :: {:ok, non_neg_integer()}
  def load_all do
    dir = storage_dir()

    if File.dir?(dir) do
      count = load_tools_from_dir(dir)
      {:ok, count}
    else
      {:ok, 0}
    end
  end

  defp load_tools_from_dir(dir) do
    dir
    |> File.ls!()
    |> Enum.filter(&String.ends_with?(&1, ".ex"))
    |> Enum.map(&String.trim_trailing(&1, ".ex"))
    |> Enum.count(fn name -> load_one_tool(dir, name) end)
  end

  defp load_one_tool(dir, name) do
    case load_one(dir, name) do
      :ok ->
        true

      {:error, reason} ->
        Logger.warning("Failed to load synthesized tool #{name}: #{inspect(reason)}")
        false
    end
  end

  @doc """
  List the names of all persisted tools on disk.
  """
  @spec list_persisted() :: [String.t()]
  def list_persisted do
    dir = storage_dir()

    if File.dir?(dir) do
      dir
      |> File.ls!()
      |> Enum.filter(&String.ends_with?(&1, ".ex"))
      |> Enum.map(&String.trim_trailing(&1, ".ex"))
    else
      []
    end
  end

  # --- Internal ---

  defp load_one(dir, name) do
    source_file = source_path(dir, name)
    meta_file = meta_path(dir, name)

    with {:ok, source} <- File.read(source_file),
         {:ok, meta_json} <- File.read(meta_file),
         {:ok, metadata} <- Jason.decode(meta_json),
         :ok <- StaticAnalyzer.validate(source),
         {:ok, module} <- compile_source(source),
         :ok <- validate_callbacks(module) do
      author = Map.get(metadata, "author_agent")
      status = metadata |> Map.get("status", "promoted") |> String.to_existing_atom()
      Registry.register(name, module, author_agent: author, status: status)
      :ok
    end
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
    required = [:name, :description, :parameters_schema, :execute]

    missing =
      Enum.reject(required, fn
        :execute -> function_exported?(module, :execute, 1)
        fun -> function_exported?(module, fun, 0)
      end)

    if missing == [] do
      :ok
    else
      {:error, {:missing_callbacks, missing}}
    end
  end

  defp source_path(dir, name), do: Path.join(dir, "#{name}.ex")
  defp meta_path(dir, name), do: Path.join(dir, "#{name}.meta.json")
end
