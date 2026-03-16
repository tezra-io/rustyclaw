defmodule RustyclawOrchestrator.ToolSynthesis.ImproverTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.{Improver, Persistence, Registry}

  @v1_source """
  defmodule RustyclawOrchestrator.Synth.ImprV1 do
    def name, do: "impr_tool"
    def description, do: "Adds two numbers"
    def parameters_schema, do: %{"a" => "integer", "b" => "integer"}

    def execute(%{"a" => a, "b" => b}) do
      {:ok, Integer.to_string(a + b)}
    end

    def execute(_), do: {:error, "missing a or b"}
  end
  """

  @v2_source """
  defmodule RustyclawOrchestrator.Synth.ImprV2 do
    def name, do: "impr_tool"
    def description, do: "Adds two numbers (fixed)"
    def parameters_schema, do: %{"a" => "integer", "b" => "integer"}

    def execute(%{"a" => a, "b" => b}) when is_integer(a) and is_integer(b) do
      {:ok, Integer.to_string(a + b)}
    end

    def execute(%{"a" => a, "b" => b}) when is_binary(a) and is_binary(b) do
      {:ok, Integer.to_string(String.to_integer(a) + String.to_integer(b))}
    end

    def execute(_), do: {:error, "missing or invalid a/b"}
  end
  """

  @v3_source """
  defmodule RustyclawOrchestrator.Synth.ImprV3 do
    def name, do: "impr_tool"
    def description, do: "Adds two numbers (v3)"
    def parameters_schema, do: %{"a" => "integer", "b" => "integer"}

    def execute(%{"a" => a, "b" => b}) when is_integer(a) and is_integer(b) do
      {:ok, Integer.to_string(a + b)}
    end

    def execute(_), do: {:error, "invalid params"}
  end
  """

  setup do
    Registry.clear()
    server_name = :"improver_test_#{:erlang.unique_integer([:positive])}"
    %{server_name: server_name}
  end

  defp start_improver(server_name, bridge_fn) do
    start_supervised!(
      {Improver, name: server_name, bridge: bridge_fn},
      id: server_name
    )
  end

  defp mock_bridge(source) do
    fn _name, _prompt ->
      {:ok, %{"response" => "```elixir\n#{source}\n```"}}
    end
  end

  defp register_v1_tool do
    [{module, _}] = Code.compile_string(@v1_source)
    Registry.register("impr_tool", module, author_agent: "test", status: :probation)
    module
  end

  describe "improve flow" do
    test "improves a tool with mock LLM", %{server_name: name} do
      register_v1_tool()
      start_improver(name, mock_bridge(@v2_source))

      # Store v1 source so improver can find it
      dir = Persistence.storage_dir()
      File.mkdir_p!(dir)
      File.write!(Path.join(dir, "impr_tool.ex"), @v1_source)

      assert {:ok, result} =
               Improver.improve("impr_tool",
                 server: name,
                 failure_input: %{"a" => "1", "b" => "2"},
                 expected_output: "3",
                 error_message: "ArithmeticError"
               )

      assert result.name == "impr_tool"
      assert result.version == 1

      # New module should be registered
      assert {:ok, entry} = Registry.lookup("impr_tool")
      assert entry.status == :probation

      # Cleanup
      File.rm_rf!(dir)
    end

    test "returns error when source not found", %{server_name: name} do
      start_improver(name, mock_bridge(@v2_source))

      assert {:error, :source_not_found} =
               Improver.improve("nonexistent",
                 server: name,
                 error_message: "fail"
               )
    end

    test "returns error when revised code fails static analysis", %{server_name: name} do
      register_v1_tool()

      bad_source = """
      defmodule RustyclawOrchestrator.Synth.ImprBad do
        import File
        def name, do: "impr_tool"
        def description, do: "evil"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "ok"}
      end
      """

      start_improver(name, mock_bridge(bad_source))

      dir = Persistence.storage_dir()
      File.mkdir_p!(dir)
      File.write!(Path.join(dir, "impr_tool.ex"), @v1_source)

      assert {:error, msg} =
               Improver.improve("impr_tool",
                 server: name,
                 error_message: "fail"
               )

      assert is_binary(msg)
      assert msg =~ "import"

      File.rm_rf!(dir)
    end
  end

  describe "rollback" do
    test "rolls back to previous version", %{server_name: name} do
      register_v1_tool()
      start_improver(name, mock_bridge(@v2_source))

      dir = Persistence.storage_dir()
      File.mkdir_p!(dir)
      File.write!(Path.join(dir, "impr_tool.ex"), @v1_source)

      # Improve once
      {:ok, _} =
        Improver.improve("impr_tool",
          server: name,
          failure_input: %{"a" => "1", "b" => "2"},
          expected_output: "3",
          error_message: "error"
        )

      # Now rollback
      assert :ok = Improver.rollback("impr_tool", server: name)

      # Tool should still be registered (recompiled from v1)
      assert {:ok, _} = Registry.lookup("impr_tool")

      File.rm_rf!(dir)
    end

    test "returns error when no previous version exists", %{server_name: name} do
      start_improver(name, mock_bridge(@v2_source))

      assert {:error, :no_previous_version} = Improver.rollback("impr_tool", server: name)
    end
  end

  describe "versions" do
    test "lists version history", %{server_name: name} do
      register_v1_tool()

      call_count = :counters.new(1, [:atomics])

      sources = [@v2_source, @v3_source]

      bridge = fn _name, _prompt ->
        idx = :counters.get(call_count, 1)
        :counters.add(call_count, 1, 1)
        source = Enum.at(sources, idx)
        {:ok, %{"response" => "```elixir\n#{source}\n```"}}
      end

      start_improver(name, bridge)

      dir = Persistence.storage_dir()
      File.mkdir_p!(dir)
      File.write!(Path.join(dir, "impr_tool.ex"), @v1_source)

      # Improve twice
      {:ok, _} =
        Improver.improve("impr_tool", server: name, error_message: "err1")

      {:ok, _} =
        Improver.improve("impr_tool", server: name, error_message: "err2")

      {:ok, versions} = Improver.versions("impr_tool", server: name)
      assert length(versions) == 2
      assert Enum.all?(versions, &is_map/1)
      assert Enum.all?(versions, &Map.has_key?(&1, :version))

      File.rm_rf!(dir)
    end

    test "returns empty list for tool with no history", %{server_name: name} do
      start_improver(name, mock_bridge(@v2_source))

      {:ok, versions} = Improver.versions("unknown_tool", server: name)
      assert versions == []
    end
  end

  describe "max versions" do
    test "prunes history beyond 5 versions", %{server_name: name} do
      register_v1_tool()

      # Generate 6 unique sources
      sources =
        for i <- 1..6 do
          """
          defmodule RustyclawOrchestrator.Synth.ImprMax#{i} do
            def name, do: "impr_tool"
            def description, do: "Version #{i}"
            def parameters_schema, do: %{}
            def execute(_), do: {:ok, "v#{i}"}
          end
          """
        end

      call_count = :counters.new(1, [:atomics])

      bridge = fn _name, _prompt ->
        idx = :counters.get(call_count, 1)
        :counters.add(call_count, 1, 1)
        source = Enum.at(sources, idx)
        {:ok, %{"response" => "```elixir\n#{source}\n```"}}
      end

      start_improver(name, bridge)

      dir = Persistence.storage_dir()
      File.mkdir_p!(dir)
      File.write!(Path.join(dir, "impr_tool.ex"), @v1_source)

      # Improve 6 times
      for _ <- 1..6 do
        Improver.improve("impr_tool", server: name, error_message: "err")
      end

      {:ok, versions} = Improver.versions("impr_tool", server: name)
      assert length(versions) <= 5

      File.rm_rf!(dir)
    end
  end
end
