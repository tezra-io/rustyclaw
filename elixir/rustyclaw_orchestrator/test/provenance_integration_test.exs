defmodule RustyclawOrchestrator.ProvenanceIntegrationTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{
    AgentCoordinator,
    AgentDefinition,
    AgentServer,
    AgentSupervisor,
    MessageProvenance,
    TraceStore
  }

  setup do
    TraceStore.clear()

    on_exit(fn ->
      for name <- AgentSupervisor.list_agents() do
        AgentSupervisor.stop_agent(name)
      end
    end)

    :ok
  end

  defp spawn_agent(name, opts \\ []) do
    def_ = %AgentDefinition{
      name: name,
      capabilities: Keyword.get(opts, :capabilities, ["test"]),
      delegates_to: Keyword.get(opts, :delegates_to, []),
      personality: "Test agent #{name}"
    }

    {:ok, _pid} = AgentSupervisor.spawn_agent(def_)
    def_
  end

  describe "AgentServer accepts provenance" do
    test "run_task with provenance records in TraceStore" do
      spawn_agent("prov-task-agent")
      prov = MessageProvenance.new(:external_user, origin_agent: "user")

      assert {:ok, _result} = AgentServer.run_task("prov-task-agent", "do stuff", prov)

      chain = TraceStore.get_chain(prov.trace_id)
      assert length(chain) == 1
      assert hd(chain).origin_agent == "user"
    end

    test "run_task without provenance still works" do
      spawn_agent("no-prov-agent")
      assert {:ok, _result} = AgentServer.run_task("no-prov-agent", "do stuff")
    end

    test "send_message with provenance records in TraceStore" do
      spawn_agent("prov-msg-agent")
      prov = MessageProvenance.new(:inter_agent, origin_agent: "sender")

      AgentServer.send_message("prov-msg-agent", "hello", prov)
      :timer.sleep(10)

      chain = TraceStore.get_chain(prov.trace_id)
      assert length(chain) == 1
    end

    test "send_message without provenance still works" do
      spawn_agent("no-prov-msg")
      AgentServer.send_message("no-prov-msg", "hello")
      :timer.sleep(10)

      state = AgentServer.get_state("no-prov-msg")
      assert length(state.history) == 1
    end
  end

  describe "AgentCoordinator propagates provenance" do
    test "delegate with provenance records child in TraceStore" do
      spawn_agent("coord-worker", capabilities: ["work"])

      prov =
        MessageProvenance.new(:external_user,
          origin_agent: "user",
          source_agent: "coordinator"
        )

      assert {:ok, _} =
               AgentCoordinator.delegate("do work",
                 capabilities: ["work"],
                 provenance: prov
               )

      chain = TraceStore.get_chain(prov.trace_id)
      # The propagated child provenance is recorded by AgentServer
      assert length(chain) == 1

      entry = hd(chain)
      assert entry.delegation_depth == 1
      assert entry.origin_agent == "user"
    end

    test "delegate without provenance still works" do
      spawn_agent("no-prov-worker", capabilities: ["work"])

      assert {:ok, _} =
               AgentCoordinator.delegate("do work", capabilities: ["work"])
    end

    test "trace_id propagation through 2-agent delegation chain" do
      spawn_agent("chain-a", capabilities: ["step1"])
      spawn_agent("chain-b", capabilities: ["step2"])

      prov = MessageProvenance.new(:external_user, origin_agent: "user")

      # First delegation hop
      {:ok, _} =
        AgentCoordinator.delegate("step 1",
          capabilities: ["step1"],
          provenance: prov
        )

      # Simulate second hop: propagate from the first child
      child1 = MessageProvenance.propagate(prov, source_agent: "chain-a")
      TraceStore.record(child1)

      {:ok, _} =
        AgentCoordinator.delegate("step 2",
          capabilities: ["step2"],
          provenance: child1
        )

      chain = TraceStore.get_chain(prov.trace_id)
      # chain-a received depth=1, child1 recorded at depth=1, chain-b received depth=2
      assert length(chain) >= 2

      depths = Enum.map(chain, & &1.delegation_depth)
      assert Enum.min(depths) >= 1
      assert Enum.max(depths) >= 2
    end

    test "fanout strategy propagates provenance to all agents" do
      spawn_agent("fan-a", capabilities: ["parallel"])
      spawn_agent("fan-b", capabilities: ["parallel"])

      prov = MessageProvenance.new(:external_user, origin_agent: "user")

      {:ok, results} =
        AgentCoordinator.delegate("parallel work",
          capabilities: ["parallel"],
          strategy: :fanout,
          provenance: prov
        )

      assert length(results) == 2

      chain = TraceStore.get_chain(prov.trace_id)
      assert length(chain) == 2
      assert Enum.all?(chain, &(&1.delegation_depth == 1))
    end
  end
end
