defmodule RustyclawOrchestrator.MessageProvenanceTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.MessageProvenance

  describe "new/2" do
    test "creates provenance with required fields" do
      prov = MessageProvenance.new(:external_user, origin_agent: "agent-a")

      assert prov.kind == :external_user
      assert prov.origin_agent == "agent-a"
      assert prov.source_agent == "agent-a"
      assert prov.delegation_depth == 0
      assert is_binary(prov.trace_id)
      assert prov.source_channel == nil
      assert prov.parent_trace_id == nil
      assert %DateTime{} = prov.timestamp
    end

    test "accepts optional source_agent" do
      prov = MessageProvenance.new(:inter_agent, origin_agent: "origin", source_agent: "sender")

      assert prov.origin_agent == "origin"
      assert prov.source_agent == "sender"
    end

    test "accepts optional source_channel" do
      prov =
        MessageProvenance.new(:external_user,
          origin_agent: "agent-a",
          source_channel: "telegram"
        )

      assert prov.source_channel == "telegram"
    end

    test "accepts optional parent_trace_id" do
      prov =
        MessageProvenance.new(:delegation,
          origin_agent: "agent-a",
          parent_trace_id: "parent-123"
        )

      assert prov.parent_trace_id == "parent-123"
    end

    test "generates unique trace_ids" do
      prov1 = MessageProvenance.new(:inter_agent, origin_agent: "a")
      prov2 = MessageProvenance.new(:inter_agent, origin_agent: "a")
      assert prov1.trace_id != prov2.trace_id
    end

    test "raises on missing origin_agent" do
      assert_raise KeyError, fn ->
        MessageProvenance.new(:inter_agent, [])
      end
    end
  end

  describe "propagate/2" do
    test "preserves trace_id and increments depth" do
      parent = MessageProvenance.new(:external_user, origin_agent: "agent-a")
      child = MessageProvenance.propagate(parent, source_agent: "agent-b")

      assert child.trace_id == parent.trace_id
      assert child.delegation_depth == 1
      assert child.origin_agent == "agent-a"
      assert child.source_agent == "agent-b"
      assert child.kind == :delegation
    end

    test "chains multiple propagations" do
      p0 = MessageProvenance.new(:external_user, origin_agent: "root")
      p1 = MessageProvenance.propagate(p0, source_agent: "mid")
      p2 = MessageProvenance.propagate(p1, source_agent: "leaf")

      assert p2.trace_id == p0.trace_id
      assert p2.delegation_depth == 2
      assert p2.origin_agent == "root"
      assert p2.source_agent == "leaf"
    end

    test "preserves source_channel from parent" do
      parent =
        MessageProvenance.new(:external_user,
          origin_agent: "agent-a",
          source_channel: "slack"
        )

      child = MessageProvenance.propagate(parent, source_agent: "agent-b")
      assert child.source_channel == "slack"
    end
  end

  describe "to_map/1 and from_map/1" do
    test "round-trips through serialization" do
      original =
        MessageProvenance.new(:inter_agent,
          origin_agent: "agent-a",
          source_agent: "agent-b",
          source_channel: "telegram",
          parent_trace_id: "parent-456"
        )

      map = MessageProvenance.to_map(original)

      assert map["kind"] == "inter_agent"
      assert map["origin_agent"] == "agent-a"
      assert map["source_agent"] == "agent-b"
      assert map["source_channel"] == "telegram"
      assert map["parent_trace_id"] == "parent-456"
      assert map["delegation_depth"] == 0
      assert is_binary(map["timestamp"])

      assert {:ok, restored} = MessageProvenance.from_map(map)
      assert restored.kind == original.kind
      assert restored.trace_id == original.trace_id
      assert restored.origin_agent == original.origin_agent
      assert restored.source_agent == original.source_agent
      assert restored.source_channel == original.source_channel
      assert restored.delegation_depth == original.delegation_depth
    end

    test "to_map serializes nil values" do
      prov = MessageProvenance.new(:internal_system, origin_agent: "sys")
      map = MessageProvenance.to_map(prov)
      assert map["source_channel"] == nil
      assert map["parent_trace_id"] == nil
    end

    test "from_map rejects invalid kind" do
      assert {:error, {:invalid_kind, "bogus"}} =
               MessageProvenance.from_map(%{"kind" => "bogus"})
    end

    test "from_map handles missing delegation_depth" do
      map = %{
        "kind" => "inter_agent",
        "trace_id" => "abc",
        "origin_agent" => "a",
        "source_agent" => "b",
        "timestamp" => DateTime.to_iso8601(DateTime.utc_now())
      }

      assert {:ok, prov} = MessageProvenance.from_map(map)
      assert prov.delegation_depth == 0
    end
  end
end
