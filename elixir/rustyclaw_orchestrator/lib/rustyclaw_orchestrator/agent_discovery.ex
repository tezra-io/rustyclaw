defmodule RustyclawOrchestrator.AgentDiscovery do
  @moduledoc """
  Scans agent definition files from disk and returns parsed definitions.

  Reads `*.md` files from the configured agents directory, parsing each
  with `AgentDefinition.from_file/1`. Malformed files are silently skipped.
  """

  alias RustyclawOrchestrator.AgentDefinition

  @doc """
  Load all agent definitions from the configured agents directory.
  Returns a map of `%{agent_name => AgentDefinition.t()}`.
  """
  @spec load_definitions() :: %{String.t() => AgentDefinition.t()}
  def load_definitions do
    load_definitions(agents_dir())
  end

  @doc """
  Load all agent definitions from the given directory.
  """
  @spec load_definitions(String.t()) :: %{String.t() => AgentDefinition.t()}
  def load_definitions(dir) do
    case File.ls(dir) do
      {:ok, files} ->
        files
        |> Enum.filter(&String.ends_with?(&1, ".md"))
        |> parse_definition_files(dir)

      {:error, _} ->
        %{}
    end
  end

  defp parse_definition_files(files, dir) do
    Enum.reduce(files, %{}, fn file, acc ->
      path = Path.join(dir, file)

      case AgentDefinition.from_file(path) do
        {:ok, definition} -> Map.put(acc, definition.name, definition)
        {:error, _} -> acc
      end
    end)
  end

  @doc "Return the configured agents directory."
  @spec agents_dir() :: String.t()
  def agents_dir do
    Application.get_env(:rustyclaw_orchestrator, :agents_dir, default_agents_dir())
  end

  defp default_agents_dir do
    Path.join(System.user_home!(), ".rustyclaw/agents")
  end
end
