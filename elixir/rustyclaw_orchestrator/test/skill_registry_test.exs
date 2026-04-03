defmodule RustyclawOrchestrator.SkillRegistryTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.SkillRegistry

  @skills_dir Path.join(
                System.tmp_dir!(),
                "rustyclaw_test_skills_#{System.unique_integer([:positive])}"
              )

  setup do
    File.rm_rf!(@skills_dir)
    File.mkdir_p!(@skills_dir)

    Application.put_env(:rustyclaw_orchestrator, :skills_dir, @skills_dir)

    on_exit(fn ->
      File.rm_rf!(@skills_dir)
      Application.delete_env(:rustyclaw_orchestrator, :skills_dir)
    end)

    :ok
  end

  defp create_skill(name, opts \\ []) do
    model = Keyword.get(opts, :model, "anthropic/claude-sonnet-4-20250514")
    capabilities = Keyword.get(opts, :capabilities, ["code"])

    caps_yaml = Enum.map_join(capabilities, "\n", &"  - #{&1}")

    content = """
    ---
    name: #{name}
    persistent: false
    model: #{model}
    capabilities:
    #{caps_yaml}
    ---

    You are a skill agent for #{name}.
    """

    skill_dir = Path.join(@skills_dir, name)
    File.mkdir_p!(skill_dir)
    File.write!(Path.join(skill_dir, "SKILL.md"), content)
    skill_dir
  end

  describe "load/1" do
    test "loads a valid skill definition" do
      create_skill("coding-skill")

      assert {:ok, definition} = SkillRegistry.load("coding-skill")
      assert definition.name == "coding-skill"
      assert definition.persistent == false
      assert definition.model == "anthropic/claude-sonnet-4-20250514"
      assert "code" in definition.capabilities
    end

    test "returns error for nonexistent skill" do
      assert {:error, reason} = SkillRegistry.load("nonexistent-skill")
      assert reason =~ "Failed to read"
    end

    test "returns error for skill without SKILL.md" do
      dir = Path.join(@skills_dir, "empty-skill")
      File.mkdir_p!(dir)

      assert {:error, reason} = SkillRegistry.load("empty-skill")
      assert reason =~ "Failed to read"
    end

    test "rejects path traversal attempts" do
      assert {:error, reason} = SkillRegistry.load("../../../etc/passwd")
      assert reason =~ "invalid skill name"
    end

    test "rejects skill names with slashes" do
      assert {:error, reason} = SkillRegistry.load("foo/bar")
      assert reason =~ "invalid skill name"
    end

    test "rejects skill names with dots" do
      assert {:error, reason} = SkillRegistry.load("foo.bar")
      assert reason =~ "invalid skill name"
    end

    test "rejects empty skill name" do
      assert {:error, reason} = SkillRegistry.load("")
      assert reason =~ "invalid skill name"
    end
  end

  describe "list/0" do
    test "returns empty list when no skills exist" do
      assert SkillRegistry.list() == []
    end

    test "lists available skill names" do
      create_skill("skill-a")
      create_skill("skill-b")

      skills = SkillRegistry.list()
      assert "skill-a" in skills
      assert "skill-b" in skills
      assert length(skills) == 2
    end

    test "ignores directories without SKILL.md" do
      create_skill("valid-skill")
      File.mkdir_p!(Path.join(@skills_dir, "empty-dir"))

      skills = SkillRegistry.list()
      assert skills == ["valid-skill"]
    end

    test "ignores non-directory files" do
      create_skill("valid-skill")
      File.write!(Path.join(@skills_dir, "stray-file.txt"), "not a skill")

      skills = SkillRegistry.list()
      assert skills == ["valid-skill"]
    end
  end

  describe "list_detailed/0" do
    test "returns skill definitions with metadata" do
      create_skill("detail-skill", capabilities: ["code", "shell"])

      details = SkillRegistry.list_detailed()
      assert length(details) == 1

      [skill] = details
      assert skill.name == "detail-skill"
      assert "code" in skill.capabilities
      assert "shell" in skill.capabilities
    end
  end
end
