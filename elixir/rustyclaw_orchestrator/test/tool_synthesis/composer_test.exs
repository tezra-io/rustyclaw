defmodule RustyclawOrchestrator.ToolSynthesis.ComposerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.{Composer, Registry}

  setup do
    Registry.clear()
    Composer.clear()

    server_name = :"composer_test_#{:erlang.unique_integer([:positive])}"

    %{server_name: server_name}
  end

  defp start_composer(server_name) do
    start_supervised!(
      {Composer, name: server_name},
      id: server_name
    )
  end

  defp register_tool(name, opts \\ []) do
    suffix = :erlang.unique_integer([:positive])
    module_name = :"Elixir.RustyclawOrchestrator.Synth.ComposerTool#{suffix}"

    Module.create(
      module_name,
      tool_body(name),
      Macro.Env.location(__ENV__)
    )

    status = Keyword.get(opts, :status, :promoted)
    Registry.register(name, module_name, author_agent: "test", status: status)
    module_name
  end

  defp tool_body(name) do
    quote do
      def name, do: unquote(name)
      def description, do: "test tool"
      def parameters_schema, do: %{}
      def execute(%{"text" => text}), do: {:ok, String.upcase(text)}
      def execute(_), do: {:error, "missing text"}
    end
  end

  describe "call_tool/2" do
    test "calls a registered tool through sandbox" do
      register_tool("upper")

      assert {:ok, "HELLO"} = Composer.call_tool("upper", %{"text" => "hello"})
    end

    test "returns error for non-existent tool" do
      assert {:error, "tool missing not found"} = Composer.call_tool("missing", %{})
    end

    test "rejects execution of suspended tool" do
      register_tool("suspended_t", status: :suspended)

      assert {:error, msg} = Composer.call_tool("suspended_t", %{"text" => "hi"})
      assert msg =~ "suspended"
    end

    test "rejects execution of deprecated tool" do
      register_tool("deprecated_t", status: :deprecated)

      assert {:error, msg} = Composer.call_tool("deprecated_t", %{"text" => "hi"})
      assert msg =~ "deprecated"
    end

    test "allows probation tools to be called" do
      register_tool("probation_t", status: :probation)

      assert {:ok, "HELLO"} = Composer.call_tool("probation_t", %{"text" => "hello"})
    end
  end

  describe "dependency tracking" do
    test "tracks dependencies", %{server_name: name} do
      start_composer(name)

      Composer.add_dependency("tool_a", "tool_b", server: name)
      Composer.add_dependency("tool_a", "tool_c", server: name)

      deps = Composer.get_dependencies("tool_a")
      assert "tool_b" in deps
      assert "tool_c" in deps
    end

    test "tracks reverse dependencies", %{server_name: name} do
      start_composer(name)

      Composer.add_dependency("tool_a", "tool_b", server: name)
      Composer.add_dependency("tool_c", "tool_b", server: name)

      dependents = Composer.get_dependents("tool_b")
      assert "tool_a" in dependents
      assert "tool_c" in dependents
    end

    test "does not duplicate dependencies", %{server_name: name} do
      start_composer(name)

      Composer.add_dependency("tool_a", "tool_b", server: name)
      Composer.add_dependency("tool_a", "tool_b", server: name)

      deps = Composer.get_dependencies("tool_a")
      assert length(deps) == 1
    end

    test "removes dependencies", %{server_name: name} do
      start_composer(name)

      Composer.add_dependency("tool_a", "tool_b", server: name)
      Composer.add_dependency("tool_a", "tool_c", server: name)
      Composer.remove_dependency("tool_a", "tool_b", server: name)

      deps = Composer.get_dependencies("tool_a")
      refute "tool_b" in deps
      assert "tool_c" in deps
    end

    test "returns empty list for tool with no dependencies", %{server_name: _name} do
      assert Composer.get_dependencies("no_deps") == []
      assert Composer.get_dependents("no_deps") == []
    end
  end

  describe "cascade deprecation" do
    test "suspends direct dependents", %{server_name: name} do
      start_composer(name)

      register_tool("base_tool")
      register_tool("dep_tool_1")
      register_tool("dep_tool_2")

      Composer.add_dependency("dep_tool_1", "base_tool", server: name)
      Composer.add_dependency("dep_tool_2", "base_tool", server: name)

      {:ok, suspended} = Composer.cascade_deprecate("base_tool", server: name)

      assert "dep_tool_1" in suspended
      assert "dep_tool_2" in suspended

      assert {:ok, %{status: :suspended}} = Registry.lookup("dep_tool_1")
      assert {:ok, %{status: :suspended}} = Registry.lookup("dep_tool_2")
    end

    test "cascades through transitive dependencies", %{server_name: name} do
      start_composer(name)

      register_tool("root")
      register_tool("mid")
      register_tool("leaf")

      # leaf depends on mid, mid depends on root
      Composer.add_dependency("mid", "root", server: name)
      Composer.add_dependency("leaf", "mid", server: name)

      {:ok, suspended} = Composer.cascade_deprecate("root", server: name)

      assert "mid" in suspended
      assert "leaf" in suspended
    end

    test "does not cascade to already-suspended tools", %{server_name: name} do
      start_composer(name)

      register_tool("base")
      register_tool("already_susp", status: :suspended)

      Composer.add_dependency("already_susp", "base", server: name)

      {:ok, suspended} = Composer.cascade_deprecate("base", server: name)

      refute "already_susp" in suspended
    end

    test "returns empty list when no dependents", %{server_name: name} do
      start_composer(name)

      {:ok, suspended} = Composer.cascade_deprecate("no_deps_tool", server: name)
      assert suspended == []
    end

    test "handles circular dependencies without infinite loop", %{server_name: name} do
      start_composer(name)

      register_tool("cycle_a")
      register_tool("cycle_b")

      Composer.add_dependency("cycle_a", "cycle_b", server: name)
      Composer.add_dependency("cycle_b", "cycle_a", server: name)

      {:ok, suspended} = Composer.cascade_deprecate("cycle_a", server: name)
      assert "cycle_b" in suspended
    end
  end
end
