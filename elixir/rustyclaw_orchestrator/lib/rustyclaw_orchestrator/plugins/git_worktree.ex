defmodule RustyclawOrchestrator.Plugins.GitWorktree do
  @moduledoc """
  Git worktree management for parallel plugin execution.

  Creates isolated working directories via `git worktree add` so multiple
  Workers can operate on the same repo without git conflicts.
  """

  require Logger

  # Clear inherited git env vars (e.g., from pre-commit hooks) so git commands
  # operate on the target repo, not the parent process's repo.
  @clean_git_env [{"GIT_DIR", ""}, {"GIT_INDEX_FILE", ""}, {"GIT_WORK_TREE", ""}]

  @doc """
  Create a new git worktree for the given repo.

  Returns `{:ok, worktree_path}` on success, `{:error, reason}` on failure.
  The worktree is created at `<repo>/.worktrees/plugin-<worker_id>` on a new branch.
  """
  @spec create_worktree(String.t(), String.t()) :: {:ok, String.t()} | {:error, String.t()}
  def create_worktree(repo_path, worker_id) do
    branch_name = "plugin-worktree-#{worker_id}"
    worktree_path = Path.join([repo_path, ".worktrees", "plugin-#{worker_id}"])

    worktree_parent = Path.dirname(worktree_path)
    File.mkdir_p(worktree_parent)

    case git_cmd(["worktree", "add", "-b", branch_name, worktree_path], repo_path) do
      {_output, 0} ->
        Logger.info("Created worktree at #{worktree_path} on branch #{branch_name}")
        {:ok, worktree_path}

      {output, _code} ->
        # Branch may already exist — try without -b
        case git_cmd(["worktree", "add", worktree_path, branch_name], repo_path) do
          {_output2, 0} ->
            Logger.info("Created worktree at #{worktree_path} on existing branch #{branch_name}")

            {:ok, worktree_path}

          {output2, _code2} ->
            Logger.error("Failed to create worktree: #{output} / #{output2}")
            {:error, "worktree creation failed: #{String.trim(output2)}"}
        end
    end
  end

  @doc """
  Remove a git worktree and clean up its branch.

  Returns `:ok` on success, `{:error, reason}` on failure.
  """
  @spec cleanup_worktree(String.t()) :: :ok | {:error, String.t()}
  def cleanup_worktree(worktree_path) do
    case git_cmd(["rev-parse", "--git-common-dir"], worktree_path) do
      {git_common_dir, 0} ->
        repo_path = git_common_dir |> String.trim() |> Path.dirname()
        do_cleanup(repo_path, worktree_path)

      {_output, _code} ->
        repo_path = worktree_path |> Path.dirname() |> Path.dirname()
        do_cleanup(repo_path, worktree_path)
    end
  end

  @doc """
  List active worktrees for a repo.
  """
  @spec list_worktrees(String.t()) :: {:ok, [String.t()]} | {:error, String.t()}
  def list_worktrees(repo_path) do
    case git_cmd(["worktree", "list", "--porcelain"], repo_path) do
      {output, 0} ->
        paths =
          output
          |> String.split("\n")
          |> Enum.filter(&String.starts_with?(&1, "worktree "))
          |> Enum.map(&String.trim_leading(&1, "worktree "))

        {:ok, paths}

      {output, _code} ->
        {:error, "failed to list worktrees: #{String.trim(output)}"}
    end
  end

  # --- Internals ---

  defp do_cleanup(repo_path, worktree_path) do
    case git_cmd(["worktree", "remove", "--force", worktree_path], repo_path) do
      {_output, 0} ->
        Logger.info("Removed worktree at #{worktree_path}")
        :ok

      {output, _code} ->
        Logger.error("Failed to remove worktree at #{worktree_path}: #{output}")
        {:error, "worktree removal failed: #{String.trim(output)}"}
    end
  end

  defp git_cmd(args, cd) do
    System.cmd("git", args, cd: cd, stderr_to_stdout: true, env: @clean_git_env)
  end
end
