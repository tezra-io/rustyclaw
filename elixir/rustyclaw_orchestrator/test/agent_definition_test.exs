defmodule RustyclawOrchestrator.AgentDefinitionTest do
  use ExUnit.Case, async: true

  alias RustyclawOrchestrator.AgentDefinition

  describe "parse/1" do
    test "parses valid definition with all fields" do
      md = """
      ---
      name: full-agent
      persistent: true
      skills:
        - twitter
        - memory
      memory: shared-read
      memory_backend: sqlite
      schedule: "0 10 * * *"
      channels:
        - telegram
      delegates_to:
        - helper
      model: gpt-4o
      temperature: 0.5
      max_tools_per_turn: 5
      allowed_tools:
        - shell
      capabilities:
        - web_search
        - summarization
      ---

      A fully configured agent.
      """

      assert {:ok, def} = AgentDefinition.parse(md)
      assert def.name == "full-agent"
      assert def.persistent == true
      assert def.skills == ["twitter", "memory"]
      assert def.memory == :shared_read
      assert def.memory_backend == "sqlite"
      assert def.schedule == "0 10 * * *"
      assert def.channels == ["telegram"]
      assert def.delegates_to == ["helper"]
      assert def.model == "gpt-4o"
      assert def.temperature == 0.5
      assert def.max_tools_per_turn == 5
      assert def.allowed_tools == ["shell"]
      assert def.capabilities == ["web_search", "summarization"]
      assert def.personality =~ "fully configured"
    end

    test "parses minimal definition with defaults" do
      md = "---\nname: test\n---\nHello"

      assert {:ok, def} = AgentDefinition.parse(md)
      assert def.name == "test"
      assert def.persistent == false
      assert def.skills == []
      assert def.memory == :isolated
      assert def.memory_backend == "markdown"
      assert def.max_tools_per_turn == 10
      assert def.allowed_tools == []
      assert def.capabilities == []
      assert def.personality == "Hello"
    end

    test "rejects missing frontmatter" do
      assert {:error, msg} = AgentDefinition.parse("No frontmatter here")
      assert msg =~ "YAML frontmatter"
    end

    test "rejects missing closing frontmatter" do
      assert {:error, msg} = AgentDefinition.parse("---\nname: test\nno closing")
      assert msg =~ "Missing closing"
    end

    test "rejects invalid YAML" do
      assert {:error, msg} = AgentDefinition.parse("---\n{{{invalid\n---\nBody")
      assert msg =~ "Invalid YAML" or msg =~ "error"
    end

    test "rejects invalid memory isolation" do
      assert {:error, msg} = AgentDefinition.parse("---\nname: test\nmemory: global\n---\n")
      assert msg =~ "Invalid memory isolation"
    end

    test "parses integer temperature as float" do
      assert {:ok, def} = AgentDefinition.parse("---\nname: test\ntemperature: 1\n---\n")
      assert def.temperature == 1.0
      assert is_float(def.temperature)
    end
  end

  describe "validate/2" do
    test "valid definition returns empty warnings" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\n---\n")
      assert {:ok, []} = AgentDefinition.validate(def)
    end

    test "rejects empty name" do
      def = %AgentDefinition{name: ""}
      assert {:error, msg} = AgentDefinition.validate(def)
      assert msg =~ "empty"
    end

    test "rejects path separators in name" do
      def = %AgentDefinition{name: "foo/bar"}
      assert {:error, msg} = AgentDefinition.validate(def)
      assert msg =~ "path separators"
    end

    test "rejects schedule without persistent" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\nschedule: \"* * * * *\"\n---\n")
      assert {:error, msg} = AgentDefinition.validate(def)
      assert msg =~ "persistent"
    end

    test "warns on unknown skills" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\nskills:\n  - nonexistent\n---\n")
      assert {:ok, warnings} = AgentDefinition.validate(def, ["twitter"])
      assert Enum.any?(warnings, &(&1 =~ "nonexistent"))
    end

    test "warns on non-snake_case capabilities" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\ncapabilities:\n  - FileRead\n---\n")
      assert {:ok, warnings} = AgentDefinition.validate(def)
      assert Enum.any?(warnings, &(&1 =~ "FileRead" and &1 =~ "snake_case"))
    end

    test "warns on hyphenated capabilities" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\ncapabilities:\n  - file-read\n---\n")
      assert {:ok, warnings} = AgentDefinition.validate(def)
      assert Enum.any?(warnings, &(&1 =~ "file-read"))
    end

    test "warns on empty capability" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\ncapabilities:\n  - \"\"\n---\n")
      assert {:ok, warnings} = AgentDefinition.validate(def)
      assert Enum.any?(warnings, &(&1 =~ "empty"))
    end

    test "no warnings for valid snake_case capabilities" do
      {:ok, def} =
        AgentDefinition.parse(
          "---\nname: test\ncapabilities:\n  - file_read\n  - web_search\n---\n"
        )

      assert {:ok, warnings} = AgentDefinition.validate(def)
      cap_warnings = Enum.filter(warnings, &(&1 =~ "snake_case" or &1 =~ "Capability"))
      assert cap_warnings == []
    end

    test "warns on unknown memory backend" do
      {:ok, def} = AgentDefinition.parse("---\nname: test\nmemory_backend: nosql\n---\n")
      assert {:ok, warnings} = AgentDefinition.validate(def)
      assert Enum.any?(warnings, &(&1 =~ "nosql"))
    end
  end

  describe "from_file/1" do
    test "returns error for nonexistent file" do
      assert {:error, msg} = AgentDefinition.from_file("/nonexistent/file.md")
      assert msg =~ "Failed to read"
    end

    test "reads and parses a valid file" do
      path = Path.join(System.tmp_dir!(), "test_agent_#{System.unique_integer([:positive])}.md")
      File.write!(path, "---\nname: file-agent\n---\nFrom a file.")
      on_exit(fn -> File.rm(path) end)

      assert {:ok, def} = AgentDefinition.from_file(path)
      assert def.name == "file-agent"
      assert def.personality == "From a file."
    end
  end
end
