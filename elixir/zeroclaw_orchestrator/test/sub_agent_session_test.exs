defmodule ZeroclawOrchestrator.SubAgentSessionTest do
  use ExUnit.Case

  alias ZeroclawOrchestrator.SubAgentSession

  setup do
    SubAgentSession.clear()
    :ok
  end

  describe "create/3" do
    test "creates a session with pending status" do
      session = SubAgentSession.create("agent-1", "summarize docs")

      assert session.agent_name == "agent-1"
      assert session.task == "summarize docs"
      assert session.status == :pending
      assert session.result == nil
      assert is_binary(session.id)
      assert %DateTime{} = session.started_at
    end

    test "creates a session with parent agent" do
      session = SubAgentSession.create("child", "subtask", parent_agent: "parent")
      assert session.parent_agent == "parent"
    end

    test "creates a session with metadata" do
      session = SubAgentSession.create("agent", "task", metadata: %{priority: :high})
      assert session.metadata == %{priority: :high}
    end
  end

  describe "lifecycle transitions" do
    test "pending → active" do
      session = SubAgentSession.create("agent", "task")
      assert {:ok, activated} = SubAgentSession.activate(session.id)
      assert activated.status == :active
    end

    test "active → completed" do
      session = SubAgentSession.create("agent", "task")
      {:ok, _} = SubAgentSession.activate(session.id)
      assert {:ok, completed} = SubAgentSession.complete(session.id, "done!")
      assert completed.status == :completed
      assert completed.result == "done!"
      assert %DateTime{} = completed.completed_at
    end

    test "active → failed" do
      session = SubAgentSession.create("agent", "task")
      {:ok, _} = SubAgentSession.activate(session.id)
      assert {:ok, failed} = SubAgentSession.fail(session.id, "timeout")
      assert failed.status == :failed
      assert failed.result == "timeout"
    end

    test "pending → cancelled" do
      session = SubAgentSession.create("agent", "task")
      assert {:ok, cancelled} = SubAgentSession.cancel(session.id)
      assert cancelled.status == :cancelled
    end

    test "completed → active is invalid" do
      session = SubAgentSession.create("agent", "task")
      {:ok, _} = SubAgentSession.complete(session.id)
      assert {:error, :invalid_transition} = SubAgentSession.activate(session.id)
    end

    test "transition on nonexistent returns not_found" do
      assert {:error, :not_found} = SubAgentSession.activate("nonexistent")
    end
  end

  describe "get/1" do
    test "returns session by ID" do
      session = SubAgentSession.create("agent", "task")
      assert {:ok, found} = SubAgentSession.get(session.id)
      assert found.id == session.id
    end

    test "returns not_found for unknown ID" do
      assert {:error, :not_found} = SubAgentSession.get("unknown")
    end
  end

  describe "list/1" do
    test "lists all sessions" do
      SubAgentSession.create("a", "task1")
      SubAgentSession.create("b", "task2")
      assert length(SubAgentSession.list()) == 2
    end

    test "filters by agent_name" do
      SubAgentSession.create("a", "task1")
      SubAgentSession.create("b", "task2")
      assert length(SubAgentSession.list(agent_name: "a")) == 1
    end

    test "filters by status" do
      s1 = SubAgentSession.create("a", "task1")
      SubAgentSession.create("a", "task2")
      SubAgentSession.activate(s1.id)

      assert length(SubAgentSession.list(status: :active)) == 1
      assert length(SubAgentSession.list(status: :pending)) == 1
    end
  end

  describe "delete/1 and clear/0" do
    test "deletes a single session" do
      session = SubAgentSession.create("agent", "task")
      SubAgentSession.delete(session.id)
      assert {:error, :not_found} = SubAgentSession.get(session.id)
    end

    test "clear removes all sessions" do
      SubAgentSession.create("a", "task1")
      SubAgentSession.create("b", "task2")
      SubAgentSession.clear()
      assert SubAgentSession.list() == []
    end
  end

  describe "count/1" do
    test "counts all sessions" do
      SubAgentSession.create("a", "task1")
      SubAgentSession.create("b", "task2")
      assert SubAgentSession.count() == 2
    end

    test "counts by status" do
      s = SubAgentSession.create("a", "task1")
      SubAgentSession.create("b", "task2")
      SubAgentSession.activate(s.id)

      assert SubAgentSession.count(status: :active) == 1
      assert SubAgentSession.count(status: :pending) == 1
    end
  end
end
