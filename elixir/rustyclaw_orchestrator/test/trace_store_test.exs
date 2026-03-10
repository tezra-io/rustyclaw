defmodule RustyclawOrchestrator.TraceStoreTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.{MessageProvenance, TraceStore}

  setup do
    TraceStore.clear()
    :ok
  end

  describe "record/1 and get_chain/1" do
    test "stores and retrieves a single entry" do
      prov = MessageProvenance.new(:inter_agent, origin_agent: "agent-a")
      assert :ok = TraceStore.record(prov)

      chain = TraceStore.get_chain(prov.trace_id)
      assert length(chain) == 1
      assert hd(chain).trace_id == prov.trace_id
    end

    test "stores multiple entries for same trace_id" do
      parent = MessageProvenance.new(:external_user, origin_agent: "root")
      child = MessageProvenance.propagate(parent, source_agent: "leaf")

      TraceStore.record(parent)
      TraceStore.record(child)

      chain = TraceStore.get_chain(parent.trace_id)
      assert length(chain) == 2
    end

    test "get_chain returns entries sorted by delegation_depth" do
      p0 = MessageProvenance.new(:external_user, origin_agent: "root")
      p1 = MessageProvenance.propagate(p0, source_agent: "mid")
      p2 = MessageProvenance.propagate(p1, source_agent: "leaf")

      # Insert out of order
      TraceStore.record(p2)
      TraceStore.record(p0)
      TraceStore.record(p1)

      chain = TraceStore.get_chain(p0.trace_id)
      depths = Enum.map(chain, & &1.delegation_depth)
      assert depths == [0, 1, 2]
    end

    test "get_chain returns empty list for unknown trace_id" do
      assert TraceStore.get_chain("nonexistent") == []
    end

    test "different trace_ids are isolated" do
      prov1 = MessageProvenance.new(:inter_agent, origin_agent: "a")
      prov2 = MessageProvenance.new(:inter_agent, origin_agent: "b")

      TraceStore.record(prov1)
      TraceStore.record(prov2)

      assert length(TraceStore.get_chain(prov1.trace_id)) == 1
      assert length(TraceStore.get_chain(prov2.trace_id)) == 1
    end
  end

  describe "cleanup_older_than/1" do
    test "removes entries older than threshold" do
      old_prov = %MessageProvenance{
        kind: :inter_agent,
        trace_id: "old-trace",
        origin_agent: "a",
        source_agent: "a",
        delegation_depth: 0,
        timestamp: DateTime.add(DateTime.utc_now(), -3600, :second)
      }

      new_prov = MessageProvenance.new(:inter_agent, origin_agent: "b")

      TraceStore.record(old_prov)
      TraceStore.record(new_prov)

      removed = TraceStore.cleanup_older_than(1800)
      assert removed == 1

      assert TraceStore.get_chain("old-trace") == []
      assert length(TraceStore.get_chain(new_prov.trace_id)) == 1
    end

    test "returns 0 when nothing to clean" do
      prov = MessageProvenance.new(:inter_agent, origin_agent: "a")
      TraceStore.record(prov)

      assert TraceStore.cleanup_older_than(3600) == 0
    end
  end

  describe "clear/0" do
    test "removes all entries" do
      prov = MessageProvenance.new(:inter_agent, origin_agent: "a")
      TraceStore.record(prov)

      assert :ok = TraceStore.clear()
      assert TraceStore.get_chain(prov.trace_id) == []
    end
  end
end
