defmodule RustyclawOrchestrator.Plugins.ContextBuilder do
  @moduledoc """
  Assembles execution context for plugin tasks.

  Reads project CLAUDE.md from the filesystem, fetches git state and recent
  commits via RustBridge, and adds coding-specific context when the plugin
  has the `:coding` capability.
  """

  require Logger

  @doc """
  Build a context map for plugin execution.

  Reads project state and optionally adds coding-specific context
  when `:coding` is in the capabilities list.
  """
  @spec build(task :: map(), capabilities :: [atom()]) :: map()
  def build(task, capabilities) do
    repo_path = Map.get(task, :repo_path) || Map.get(task, "repo_path")

    base = %{
      claude_md: read_claude_md(repo_path),
      git_state: fetch_git_state(repo_path),
      recent_commits: fetch_recent_commits(repo_path)
    }

    if :coding in capabilities do
      Map.put(
        base,
        :task_description,
        Map.get(task, :description) || Map.get(task, "description")
      )
    else
      base
    end
  end

  defp read_claude_md(nil), do: nil

  defp read_claude_md(repo_path) do
    path = Path.join(repo_path, "CLAUDE.md")

    case File.read(path) do
      {:ok, content} -> content
      {:error, _} -> nil
    end
  end

  defp fetch_git_state(nil), do: nil

  defp fetch_git_state(repo_path) do
    case RustyclawOrchestrator.RustBridge.run_task("system", "git_status",
           model: nil,
           repo_path: repo_path
         ) do
      {:ok, result} ->
        result

      {:error, reason} ->
        Logger.debug("Failed to fetch git state: #{inspect(reason)}")
        nil
    end
  rescue
    _ -> nil
  catch
    :exit, _ -> nil
  end

  defp fetch_recent_commits(nil), do: nil

  defp fetch_recent_commits(repo_path) do
    case RustyclawOrchestrator.RustBridge.run_task("system", "git_log",
           model: nil,
           repo_path: repo_path,
           limit: 10
         ) do
      {:ok, result} ->
        result

      {:error, reason} ->
        Logger.debug("Failed to fetch recent commits: #{inspect(reason)}")
        nil
    end
  rescue
    _ -> nil
  catch
    :exit, _ -> nil
  end
end
