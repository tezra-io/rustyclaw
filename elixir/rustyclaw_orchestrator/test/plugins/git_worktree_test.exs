defmodule RustyclawOrchestrator.Plugins.GitWorktreeTest do
  use ExUnit.Case, async: false

  alias RustyclawOrchestrator.Plugins.GitWorktree

  # Clear inherited git env vars (e.g., GIT_DIR from pre-commit hooks)
  @clean_git_env [{"GIT_DIR", ""}, {"GIT_INDEX_FILE", ""}, {"GIT_WORK_TREE", ""}]

  setup do
    tmp_dir =
      Path.join(System.tmp_dir!(), "git_worktree_test_#{System.unique_integer([:positive])}")

    repo_path = Path.join(tmp_dir, "test_repo")
    File.mkdir_p!(repo_path)

    git_cmd = fn args -> System.cmd("git", args, cd: repo_path, env: @clean_git_env) end

    git_cmd.(["init"])
    git_cmd.(["config", "user.email", "test@test.com"])
    git_cmd.(["config", "user.name", "Test"])

    # Create an initial commit (worktrees need at least one commit)
    readme = Path.join(repo_path, "README.md")
    File.write!(readme, "# Test Repo")
    git_cmd.(["add", "."])
    git_cmd.(["commit", "-m", "init"])

    on_exit(fn -> File.rm_rf!(tmp_dir) end)

    %{repo_path: repo_path}
  end

  describe "create_worktree/2" do
    test "creates a worktree in .worktrees directory", %{repo_path: repo_path} do
      worker_id = "test-#{System.unique_integer([:positive])}"

      assert {:ok, worktree_path} = GitWorktree.create_worktree(repo_path, worker_id)
      assert String.contains?(worktree_path, ".worktrees/plugin-#{worker_id}")
      assert File.dir?(worktree_path)

      # Verify worktree has the repo content
      assert File.exists?(Path.join(worktree_path, "README.md"))

      # Cleanup
      GitWorktree.cleanup_worktree(worktree_path)
    end

    test "creates worktree with unique branch", %{repo_path: repo_path} do
      worker_id = "branch-#{System.unique_integer([:positive])}"

      assert {:ok, worktree_path} = GitWorktree.create_worktree(repo_path, worker_id)

      # Verify the branch was created
      {branches, 0} =
        System.cmd("git", ["branch", "--list"], cd: repo_path, env: @clean_git_env)

      assert String.contains?(branches, "plugin-worktree-#{worker_id}")

      # Cleanup
      GitWorktree.cleanup_worktree(worktree_path)
    end

    test "returns error for non-existent repo" do
      assert {:error, _reason} = GitWorktree.create_worktree("/nonexistent/path", "wk-1")
    end
  end

  describe "cleanup_worktree/1" do
    test "removes worktree and directory", %{repo_path: repo_path} do
      worker_id = "cleanup-#{System.unique_integer([:positive])}"
      {:ok, worktree_path} = GitWorktree.create_worktree(repo_path, worker_id)

      assert File.dir?(worktree_path)
      assert :ok = GitWorktree.cleanup_worktree(worktree_path)
      refute File.dir?(worktree_path)
    end

    test "returns error for non-existent worktree" do
      assert {:error, _reason} = GitWorktree.cleanup_worktree("/nonexistent/worktree")
    end
  end

  describe "list_worktrees/1" do
    test "lists worktrees for a repo", %{repo_path: repo_path} do
      assert {:ok, paths} = GitWorktree.list_worktrees(repo_path)
      # Main worktree is always listed
      assert paths != []
    end

    test "includes created worktrees", %{repo_path: repo_path} do
      worker_id = "list-#{System.unique_integer([:positive])}"
      {:ok, worktree_path} = GitWorktree.create_worktree(repo_path, worker_id)

      assert {:ok, paths} = GitWorktree.list_worktrees(repo_path)
      assert Enum.any?(paths, &String.contains?(&1, "plugin-#{worker_id}"))

      GitWorktree.cleanup_worktree(worktree_path)
    end
  end
end
