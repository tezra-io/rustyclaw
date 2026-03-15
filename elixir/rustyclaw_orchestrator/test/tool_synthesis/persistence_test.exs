defmodule RustyclawOrchestrator.ToolSynthesis.PersistenceTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Registry}

  @valid_source """
  defmodule RustyclawOrchestrator.Synth.PersistTestTool do
    def name, do: "persist_test_tool"
    def description, do: "A tool for persistence testing"
    def parameters_schema, do: %{"input" => "string"}

    def execute(%{"input" => input}) do
      {:ok, String.upcase(input)}
    end

    def execute(_), do: {:ok, "default"}
  end
  """

  setup do
    # Use a unique temp directory per test to avoid conflicts
    dir =
      Path.join(
        System.tmp_dir!(),
        "rustyclaw_persist_test_#{:erlang.unique_integer([:positive])}"
      )

    File.mkdir_p!(dir)
    Application.put_env(:rustyclaw_orchestrator, :synthesized_tools_dir, dir)

    # Clean up the registry between tests
    Registry.clear()

    on_exit(fn ->
      File.rm_rf!(dir)
      Application.delete_env(:rustyclaw_orchestrator, :synthesized_tools_dir)
    end)

    %{dir: dir}
  end

  describe "save/3" do
    test "saves source and metadata files to disk", %{dir: dir} do
      metadata = %{author_agent: "test_agent", status: "promoted"}
      assert :ok = Persistence.save("my_tool", @valid_source, metadata)

      assert File.exists?(Path.join(dir, "my_tool.ex"))
      assert File.exists?(Path.join(dir, "my_tool.meta.json"))

      assert File.read!(Path.join(dir, "my_tool.ex")) == @valid_source

      meta = dir |> Path.join("my_tool.meta.json") |> File.read!() |> Jason.decode!()
      assert meta["author_agent"] == "test_agent"
      assert meta["status"] == "promoted"
    end

    test "creates directory if it doesn't exist" do
      nested =
        Path.join(
          System.tmp_dir!(),
          "rustyclaw_nested_#{:erlang.unique_integer([:positive])}/deep"
        )

      Application.put_env(:rustyclaw_orchestrator, :synthesized_tools_dir, nested)

      on_exit(fn -> File.rm_rf!(Path.dirname(nested)) end)

      assert :ok = Persistence.save("tool", @valid_source, %{})
      assert File.exists?(Path.join(nested, "tool.ex"))
    end
  end

  describe "delete/1" do
    test "removes source and metadata files", %{dir: dir} do
      Persistence.save("to_delete", @valid_source, %{status: "promoted"})
      assert File.exists?(Path.join(dir, "to_delete.ex"))

      assert :ok = Persistence.delete("to_delete")
      refute File.exists?(Path.join(dir, "to_delete.ex"))
      refute File.exists?(Path.join(dir, "to_delete.meta.json"))
    end

    test "succeeds even if files don't exist" do
      assert :ok = Persistence.delete("nonexistent")
    end
  end

  describe "load_all/0" do
    test "loads and registers a valid persisted tool" do
      Persistence.save("load_test", @valid_source, %{
        "author_agent" => "agent_a",
        "status" => "promoted"
      })

      assert {:ok, 1} = Persistence.load_all()
      assert {:ok, entry} = Registry.lookup("load_test")
      assert entry.module == RustyclawOrchestrator.Synth.PersistTestTool
      assert entry.status == :promoted
      assert entry.author_agent == "agent_a"
    end

    test "returns 0 when directory is empty", %{dir: dir} do
      # dir exists but is empty
      assert File.dir?(dir)
      assert {:ok, 0} = Persistence.load_all()
    end

    test "returns 0 when directory doesn't exist" do
      Application.put_env(
        :rustyclaw_orchestrator,
        :synthesized_tools_dir,
        Path.join(System.tmp_dir!(), "nonexistent_#{:erlang.unique_integer([:positive])}")
      )

      assert {:ok, 0} = Persistence.load_all()
    end

    test "skips tools that fail static analysis" do
      bad_source = """
      defmodule RustyclawOrchestrator.Synth.BadPersisted do
        import Enum
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      Persistence.save("bad_tool", bad_source, %{"status" => "promoted"})

      assert {:ok, 0} = Persistence.load_all()
    end

    test "skips tools with invalid source" do
      File.write!(Path.join(Persistence.storage_dir(), "broken.ex"), "defmodule do {{{")

      File.write!(
        Path.join(Persistence.storage_dir(), "broken.meta.json"),
        Jason.encode!(%{"status" => "promoted"})
      )

      assert {:ok, 0} = Persistence.load_all()
    end
  end

  describe "list_persisted/0" do
    test "lists tool names from disk", %{dir: _dir} do
      Persistence.save("tool_a", @valid_source, %{})
      Persistence.save("tool_b", @valid_source, %{})

      names = Persistence.list_persisted()
      assert "tool_a" in names
      assert "tool_b" in names
    end

    test "returns empty list when no tools exist" do
      assert Persistence.list_persisted() == []
    end
  end
end
