defmodule RustyclawOrchestrator.Plugins.ContextBuilderTest do
  use ExUnit.Case, async: true

  alias RustyclawOrchestrator.Plugins.ContextBuilder

  describe "build/2" do
    test "returns context map with expected keys" do
      task = %{description: "fix bug", repo_path: "/nonexistent/path"}
      context = ContextBuilder.build(task, [:coding])

      assert is_map(context)
      assert Map.has_key?(context, :claude_md)
      assert Map.has_key?(context, :git_state)
      assert Map.has_key?(context, :recent_commits)
    end

    test "includes task_description for coding capability" do
      task = %{description: "add feature", repo_path: "/nonexistent/path"}
      context = ContextBuilder.build(task, [:coding])

      assert Map.has_key?(context, :task_description)
      assert context.task_description == "add feature"
    end

    test "omits task_description for non-coding capability" do
      task = %{description: "review docs", repo_path: "/nonexistent/path"}
      context = ContextBuilder.build(task, [:review])

      refute Map.has_key?(context, :task_description)
    end

    test "handles nil repo_path gracefully" do
      task = %{description: "no repo"}
      context = ContextBuilder.build(task, [:coding])

      assert is_map(context)
      assert context.claude_md == nil
      assert context.git_state == nil
      assert context.recent_commits == nil
    end

    test "reads CLAUDE.md from valid repo path" do
      # Create a temp directory with a CLAUDE.md
      tmp_dir =
        Path.join(System.tmp_dir!(), "ctx_builder_test_#{:erlang.unique_integer([:positive])}")

      File.mkdir_p!(tmp_dir)
      claude_md_content = "# Test Project\nSome instructions."
      File.write!(Path.join(tmp_dir, "CLAUDE.md"), claude_md_content)

      task = %{description: "test", repo_path: tmp_dir}
      context = ContextBuilder.build(task, [])

      assert context.claude_md == claude_md_content

      File.rm_rf!(tmp_dir)
    end

    test "returns nil claude_md when file doesn't exist" do
      task = %{description: "test", repo_path: "/definitely/not/a/real/path"}
      context = ContextBuilder.build(task, [])

      assert context.claude_md == nil
    end

    test "handles string keys in task map" do
      task = %{"description" => "string keys", "repo_path" => "/nonexistent"}
      context = ContextBuilder.build(task, [:coding])

      assert is_map(context)
      assert context.task_description == "string keys"
    end
  end
end
