defmodule RustyclawOrchestrator.ToolSynthesis.SynthesizerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.{Registry, Synthesizer}

  @good_source """
  defmodule RustyclawOrchestrator.Synth.Upcase do
    def name, do: "upcase"
    def description, do: "Uppercases input text"
    def parameters_schema, do: %{"text" => "string"}

    def execute(%{"text" => text}) do
      {:ok, String.upcase(text)}
    end

    def execute(_), do: {:error, "missing text parameter"}
  end
  """

  setup do
    Registry.clear()

    # Unique server name per test to avoid conflicts
    server_name = :"synth_test_#{:erlang.unique_integer([:positive])}"

    %{server_name: server_name}
  end

  defp start_synthesizer(server_name, bridge_fn) do
    start_supervised!(
      {Synthesizer, name: server_name, bridge: bridge_fn},
      id: server_name
    )
  end

  defp mock_bridge(source) do
    fn _name, _prompt ->
      {:ok, %{"response" => "```elixir\n#{source}\n```"}}
    end
  end

  describe "full synthesis flow" do
    test "synthesizes, validates, compiles, and registers a tool", %{server_name: name} do
      start_synthesizer(name, mock_bridge(@good_source))

      request = %{capability: "uppercase text", suggested_name: "upcase"}

      assert {:ok, result} =
               Synthesizer.synthesize(request, server: name, agent_id: "agent_1")

      assert result.name == "upcase"
      assert result.status == :probation
      assert result.module == RustyclawOrchestrator.Synth.Upcase
      assert is_binary(result.source)

      # Tool is registered
      assert {:ok, entry} = Registry.lookup("upcase")
      assert entry.status == :probation
      assert entry.author_agent == "agent_1"
    end

    test "synthesized tool is executable", %{server_name: name} do
      start_synthesizer(name, mock_bridge(@good_source))

      request = %{capability: "uppercase text", suggested_name: "upcase"}
      {:ok, result} = Synthesizer.synthesize(request, server: name, agent_id: "a")

      assert {:ok, "HELLO"} = result.module.execute(%{"text" => "hello"})
    end
  end

  describe "example-based testing" do
    test "passes when output matches expected", %{server_name: name} do
      start_synthesizer(name, mock_bridge(@good_source))

      request = %{capability: "uppercase text", suggested_name: "upcase"}

      assert {:ok, _} =
               Synthesizer.synthesize(request,
                 server: name,
                 agent_id: "a",
                 input_example: %{"text" => "hello"},
                 expected_output: "HELLO"
               )
    end

    test "fails when output doesn't match expected", %{server_name: name} do
      start_synthesizer(name, mock_bridge(@good_source))

      request = %{capability: "uppercase text", suggested_name: "upcase"}

      assert {:error, {:example_mismatch, _}} =
               Synthesizer.synthesize(request,
                 server: name,
                 agent_id: "a",
                 input_example: %{"text" => "hello"},
                 expected_output: "wrong"
               )
    end
  end

  describe "rate limiting" do
    test "allows up to 3 synthesis attempts per agent per hour", %{server_name: name} do
      # Generate unique module names to avoid :already_exists in registry
      sources =
        for i <- 1..3 do
          """
          defmodule RustyclawOrchestrator.Synth.RateTest#{i} do
            def name, do: "rate_test_#{i}"
            def description, do: "Rate test #{i}"
            def parameters_schema, do: %{}
            def execute(_), do: {:ok, "ok"}
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

      start_synthesizer(name, bridge)

      for i <- 1..3 do
        request = %{capability: "test", suggested_name: "rate_test_#{i}"}
        assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "limited_agent")
      end

      # 4th attempt should be rate limited
      request = %{capability: "test", suggested_name: "rate_test_4"}

      assert {:error, :rate_limited} =
               Synthesizer.synthesize(request, server: name, agent_id: "limited_agent")
    end

    test "rate limits are per agent", %{server_name: name} do
      sources =
        for i <- 1..4 do
          """
          defmodule RustyclawOrchestrator.Synth.PerAgent#{i} do
            def name, do: "per_agent_#{i}"
            def description, do: "Test #{i}"
            def parameters_schema, do: %{}
            def execute(_), do: {:ok, "ok"}
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

      start_synthesizer(name, bridge)

      # Agent A uses 3 attempts
      for i <- 1..3 do
        request = %{capability: "test", suggested_name: "per_agent_#{i}"}
        assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "agent_a")
      end

      # Agent B still has quota
      request = %{capability: "test", suggested_name: "per_agent_4"}
      assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "agent_b")
    end
  end

  describe "static analysis rejection" do
    test "rejects code that fails static analysis", %{server_name: name} do
      bad_source = """
      defmodule RustyclawOrchestrator.Synth.Evil do
        import File
        def name, do: "evil"
        def description, do: "evil"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, File.read!("/etc/passwd")}
      end
      """

      start_synthesizer(name, mock_bridge(bad_source))

      request = %{capability: "evil", suggested_name: "evil"}

      assert {:error, msg} = Synthesizer.synthesize(request, server: name, agent_id: "a")
      assert is_binary(msg)
      assert msg =~ "import"
    end
  end

  describe "compilation failure handling" do
    test "returns error for code that doesn't compile", %{server_name: name} do
      bad_source = """
      defmodule RustyclawOrchestrator.Synth.Broken do
        def name, do: "broken"
        def execute(_), do: {:ok, undefined_var}
      end
      """

      start_synthesizer(name, mock_bridge(bad_source))

      request = %{capability: "broken", suggested_name: "broken"}

      assert {:error, {:compilation_error, _}} =
               Synthesizer.synthesize(request, server: name, agent_id: "a")
    end
  end

  describe "behaviour validation" do
    test "rejects module missing required callbacks", %{server_name: name} do
      incomplete_source = """
      defmodule RustyclawOrchestrator.Synth.Incomplete do
        def name, do: "incomplete"
        def execute(_), do: {:ok, "ok"}
      end
      """

      start_synthesizer(name, mock_bridge(incomplete_source))

      request = %{capability: "incomplete", suggested_name: "incomplete"}

      assert {:error, {:missing_callbacks, missing}} =
               Synthesizer.synthesize(request, server: name, agent_id: "a")

      assert "description/0" in missing
      assert "parameters_schema/0" in missing
    end
  end

  describe "LLM response handling" do
    test "extracts source from code fences", %{server_name: name} do
      start_synthesizer(name, mock_bridge(@good_source))

      request = %{capability: "test", suggested_name: "upcase"}
      assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "a")
    end

    test "handles response without code fences", %{server_name: name} do
      bridge = fn _name, _prompt ->
        {:ok, %{"response" => @good_source}}
      end

      start_synthesizer(name, bridge)

      request = %{capability: "test", suggested_name: "upcase"}
      assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "a")
    end

    test "handles plain text response key", %{server_name: name} do
      bridge = fn _name, _prompt ->
        {:ok, %{"text" => "```elixir\n#{@good_source}\n```"}}
      end

      start_synthesizer(name, bridge)

      request = %{capability: "test", suggested_name: "upcase"}
      assert {:ok, _} = Synthesizer.synthesize(request, server: name, agent_id: "a")
    end

    test "handles bridge error", %{server_name: name} do
      bridge = fn _name, _prompt -> {:error, :connection_refused} end

      start_synthesizer(name, bridge)

      request = %{capability: "test", suggested_name: "fail"}

      assert {:error, :connection_refused} =
               Synthesizer.synthesize(request, server: name, agent_id: "a")
    end

    test "handles response with no source code", %{server_name: name} do
      bridge = fn _name, _prompt ->
        {:ok, %{"response" => "I cannot generate that tool."}}
      end

      start_synthesizer(name, bridge)

      request = %{capability: "test", suggested_name: "nosource"}

      assert {:error, :no_source_in_response} =
               Synthesizer.synthesize(request, server: name, agent_id: "a")
    end
  end

  describe "duplicate name handling" do
    test "returns :already_exists when tool name is taken", %{server_name: name} do
      # Register a tool first
      defmodule RustyclawOrchestrator.Synth.Existing do
        def name, do: "existing"
        def description, do: "already here"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "ok"}
      end

      Registry.register("dupe_name", RustyclawOrchestrator.Synth.Existing)

      source = """
      defmodule RustyclawOrchestrator.Synth.DupeName do
        def name, do: "dupe_name"
        def description, do: "dupe"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "ok"}
      end
      """

      start_synthesizer(name, mock_bridge(source))

      request = %{capability: "test", suggested_name: "dupe_name"}

      assert {:error, :already_exists} =
               Synthesizer.synthesize(request, server: name, agent_id: "a")
    end
  end
end
