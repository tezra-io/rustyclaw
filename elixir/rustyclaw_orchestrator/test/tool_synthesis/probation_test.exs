defmodule RustyclawOrchestrator.ToolSynthesis.ProbationTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.{Persistence, Probation, Registry}

  @tool_source """
  defmodule RustyclawOrchestrator.Synth.ProbTest do
    def name, do: "prob_test"
    def description, do: "Probation test tool"
    def parameters_schema, do: %{}
    def execute(_), do: {:ok, "ok"}
  end
  """

  setup do
    Registry.clear()

    server_name = :"probation_test_#{:erlang.unique_integer([:positive])}"

    %{server_name: server_name}
  end

  defp start_probation(server_name, config \\ []) do
    start_supervised!(
      {Probation, name: server_name, config: config},
      id: server_name
    )
  end

  defp register_tool(name, opts \\ []) do
    # Compile a simple tool module with a unique name to avoid conflicts
    suffix = :erlang.unique_integer([:positive])
    module_name = :"Elixir.RustyclawOrchestrator.Synth.ProbTool#{suffix}"

    Module.create(
      module_name,
      quote do
        def name, do: unquote(name)
        def description, do: "test tool"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "ok"}
      end,
      Macro.Env.location(__ENV__)
    )

    status = Keyword.get(opts, :status, :probation)
    Registry.register(name, module_name, author_agent: "test_agent", status: status)
    module_name
  end

  defp simulate_invocations(server, tool_name, successes, failures, opts \\ []) do
    success_results =
      for _ <- 1..successes//1 do
        Registry.update_metrics(tool_name, true, 100)
        Probation.record_invocation(tool_name, true, Keyword.merge([server: server], opts))
      end

    failure_results =
      for _ <- 1..failures//1 do
        Registry.update_metrics(tool_name, false, 100)
        Probation.record_invocation(tool_name, false, Keyword.merge([server: server], opts))
      end

    success_results ++ failure_results
  end

  describe "auto-deprecation" do
    test "deprecates tool with success rate below 50% after probation period", %{
      server_name: name
    } do
      start_probation(name, probation_invocations: 10, deprecation_threshold: 0.5)
      register_tool("depr_tool")

      # 3 successes, 7 failures = 30% success rate
      results = simulate_invocations(name, "depr_tool", 3, 7)

      assert {:transition, :deprecated} in results

      # Tool should be unloaded from registry
      assert {:error, :not_found} = Registry.lookup("depr_tool")
    end

    test "does not deprecate before probation_invocations threshold", %{server_name: name} do
      start_probation(name, probation_invocations: 10, deprecation_threshold: 0.5)
      register_tool("early_tool")

      # 2 successes, 5 failures = below threshold but only 7 invocations
      results = simulate_invocations(name, "early_tool", 2, 5)

      refute {:transition, :deprecated} in results
      assert {:ok, entry} = Registry.lookup("early_tool")
      assert entry.status == :probation
    end
  end

  describe "auto-suspension" do
    test "suspends tool on crash", %{server_name: name} do
      start_probation(name)
      register_tool("crash_tool")

      Registry.update_metrics("crash_tool", false, 100)
      result = Probation.record_invocation("crash_tool", false, server: name, crash: true)

      assert result == {:transition, :suspended}
      assert {:ok, entry} = Registry.lookup("crash_tool")
      assert entry.status == :suspended
    end

    test "suspends promoted tool on crash", %{server_name: name} do
      start_probation(name)
      register_tool("promoted_crash", status: :promoted)

      Registry.update_metrics("promoted_crash", false, 100)
      result = Probation.record_invocation("promoted_crash", false, server: name, crash: true)

      assert result == {:transition, :suspended}
      assert {:ok, entry} = Registry.lookup("promoted_crash")
      assert entry.status == :suspended
    end
  end

  describe "auto-promotion" do
    test "promotes tool when success rate meets threshold with auto_promote=true", %{
      server_name: name
    } do
      start_probation(name,
        probation_invocations: 10,
        min_success_rate: 0.8,
        auto_promote: true
      )

      register_tool("promo_tool")

      # 9 successes, 1 failure = 90% success rate
      results = simulate_invocations(name, "promo_tool", 9, 1, source: @tool_source)

      assert {:transition, :promoted} in results

      assert {:ok, entry} = Registry.lookup("promo_tool")
      assert entry.status == :promoted
    end

    test "does not promote when auto_promote=false", %{server_name: name} do
      start_probation(name,
        probation_invocations: 10,
        min_success_rate: 0.8,
        auto_promote: false
      )

      register_tool("no_promo_tool")

      # 10 successes = 100% rate but auto_promote off
      results = simulate_invocations(name, "no_promo_tool", 10, 0)

      refute {:transition, :promoted} in results
      assert {:ok, entry} = Registry.lookup("no_promo_tool")
      assert entry.status == :probation
    end

    test "does not promote when success rate is below threshold", %{server_name: name} do
      start_probation(name,
        probation_invocations: 10,
        min_success_rate: 0.8,
        auto_promote: true,
        deprecation_threshold: 0.0
      )

      register_tool("low_rate_tool")

      # 7 successes, 3 failures = 70% (below 80%)
      results = simulate_invocations(name, "low_rate_tool", 7, 3)

      refute {:transition, :promoted} in results
      assert {:ok, entry} = Registry.lookup("low_rate_tool")
      assert entry.status == :probation
    end

    test "persists tool source on promotion", %{server_name: name} do
      dir = Persistence.storage_dir()
      File.rm_rf!(dir)

      start_probation(name,
        probation_invocations: 5,
        min_success_rate: 0.8,
        auto_promote: true
      )

      register_tool("persist_tool")

      # 5 successes = 100% rate
      simulate_invocations(name, "persist_tool", 5, 0, source: @tool_source)

      # Check that source was persisted
      source_path = Path.join(dir, "persist_tool.ex")
      meta_path = Path.join(dir, "persist_tool.meta.json")

      assert File.exists?(source_path)
      assert File.exists?(meta_path)

      # Cleanup
      File.rm_rf!(dir)
    end
  end

  describe "post-promotion sliding window monitoring" do
    test "suspends promoted tool when failure rate spikes above 50% in window", %{
      server_name: name
    } do
      start_probation(name, sliding_window_size: 10)
      register_tool("window_tool", status: :promoted)

      # 4 successes followed by 6 failures = 60% failure rate in window
      for _ <- 1..4 do
        Registry.update_metrics("window_tool", true, 50)
        Probation.record_invocation("window_tool", true, server: name)
      end

      results =
        for _ <- 1..6 do
          Registry.update_metrics("window_tool", false, 50)
          Probation.record_invocation("window_tool", false, server: name)
        end

      assert {:transition, :suspended} in results
      assert {:ok, entry} = Registry.lookup("window_tool")
      assert entry.status == :suspended
    end

    test "does not suspend promoted tool with acceptable failure rate", %{server_name: name} do
      start_probation(name, sliding_window_size: 10)
      register_tool("stable_tool", status: :promoted)

      # 8 successes, 2 failures = 20% failure rate
      for _ <- 1..8 do
        Registry.update_metrics("stable_tool", true, 50)
        Probation.record_invocation("stable_tool", true, server: name)
      end

      results =
        for _ <- 1..2 do
          Registry.update_metrics("stable_tool", false, 50)
          Probation.record_invocation("stable_tool", false, server: name)
        end

      refute {:transition, :suspended} in results
      assert {:ok, entry} = Registry.lookup("stable_tool")
      assert entry.status == :promoted
    end

    test "window only considers last N invocations", %{server_name: name} do
      start_probation(name, sliding_window_size: 5)
      register_tool("window_size_tool", status: :promoted)

      # 10 successes to fill window with good results
      for _ <- 1..10 do
        Registry.update_metrics("window_size_tool", true, 50)
        Probation.record_invocation("window_size_tool", true, server: name)
      end

      # 3 failures — window of 5 has [F, F, F, S, S] = 60% failure
      results =
        for _ <- 1..3 do
          Registry.update_metrics("window_size_tool", false, 50)
          Probation.record_invocation("window_size_tool", false, server: name)
        end

      # 3 out of 5 is exactly 60% > 50%, should suspend
      assert {:transition, :suspended} in results
    end
  end

  describe "deprecated tools are unloaded" do
    test "tool is removed from registry on deprecation", %{server_name: name} do
      start_probation(name, probation_invocations: 5, deprecation_threshold: 0.5)
      register_tool("unload_tool")

      # 1 success, 4 failures = 20% success rate
      simulate_invocations(name, "unload_tool", 1, 4)

      assert {:error, :not_found} = Registry.lookup("unload_tool")
    end
  end

  describe "get_state" do
    test "returns tool probation state", %{server_name: name} do
      start_probation(name)
      register_tool("state_tool")

      Registry.update_metrics("state_tool", true, 100)
      Probation.record_invocation("state_tool", true, server: name)

      assert {:ok, info} = Probation.get_state("state_tool", server: name)
      assert info.status == :probation
      assert info.invocation_count == 1
      assert info.success_rate == 1.0
    end

    test "returns error for unknown tool", %{server_name: name} do
      start_probation(name)

      assert {:error, :not_tracked} = Probation.get_state("nonexistent", server: name)
    end
  end

  describe "ignored statuses" do
    test "does nothing for already suspended tools (no crash)", %{server_name: name} do
      start_probation(name)
      register_tool("suspended_tool", status: :suspended)

      Registry.update_metrics("suspended_tool", true, 100)
      result = Probation.record_invocation("suspended_tool", true, server: name)

      assert result == :ok
    end

    test "does nothing for already deprecated tools", %{server_name: name} do
      start_probation(name)
      register_tool("deprecated_tool", status: :deprecated)

      Registry.update_metrics("deprecated_tool", true, 100)
      result = Probation.record_invocation("deprecated_tool", true, server: name)

      assert result == :ok
    end
  end
end
