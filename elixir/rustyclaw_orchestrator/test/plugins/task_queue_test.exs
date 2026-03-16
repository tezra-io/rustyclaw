defmodule RustyclawOrchestrator.Plugins.TaskQueueTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.TaskQueue

  setup do
    ets_table = :"tq_test_#{:erlang.unique_integer([:positive])}"
    queue_name = :"tq_#{:erlang.unique_integer([:positive])}"

    pid =
      start_supervised!(
        {TaskQueue,
         name: queue_name, poll_interval_ms: 0, auto_assign: false, ets_table: ets_table}
      )

    %{queue: queue_name, pid: pid, ets_table: ets_table}
  end

  describe "push_task/1 and pop_task/0" do
    test "pushes and pops a task", ctx do
      task = %{id: "t1", identifier: "TEZ-100", priority: 2, labels: []}
      assert :ok = TaskQueue.push_task(task, server: ctx.queue)

      assert {:ok, popped} = TaskQueue.pop_task(server: ctx.queue)
      assert popped.identifier == "TEZ-100"
    end

    test "returns :empty when queue is empty", ctx do
      assert :empty = TaskQueue.pop_task(server: ctx.queue)
    end
  end

  describe "priority ordering" do
    test "pops highest priority (lowest number) first", ctx do
      TaskQueue.push_task(%{id: "low", identifier: "TEZ-1", priority: 4, labels: []},
        server: ctx.queue
      )

      TaskQueue.push_task(%{id: "urgent", identifier: "TEZ-2", priority: 1, labels: []},
        server: ctx.queue
      )

      TaskQueue.push_task(%{id: "medium", identifier: "TEZ-3", priority: 2, labels: []},
        server: ctx.queue
      )

      {:ok, first} = TaskQueue.pop_task(server: ctx.queue)
      {:ok, second} = TaskQueue.pop_task(server: ctx.queue)
      {:ok, third} = TaskQueue.pop_task(server: ctx.queue)

      assert first.priority == 1
      assert second.priority == 2
      assert third.priority == 4
    end

    test "maintains insertion order for same priority", ctx do
      TaskQueue.push_task(%{id: "a", identifier: "TEZ-10", priority: 2, labels: []},
        server: ctx.queue
      )

      TaskQueue.push_task(%{id: "b", identifier: "TEZ-11", priority: 2, labels: []},
        server: ctx.queue
      )

      {:ok, first} = TaskQueue.pop_task(server: ctx.queue)
      {:ok, second} = TaskQueue.pop_task(server: ctx.queue)

      assert first.identifier == "TEZ-10"
      assert second.identifier == "TEZ-11"
    end
  end

  describe "deduplication" do
    test "pop marks task as in-progress", ctx do
      TaskQueue.push_task(%{id: "t1", identifier: "TEZ-200", priority: 1, labels: []},
        server: ctx.queue
      )

      {:ok, _} = TaskQueue.pop_task(server: ctx.queue)

      status = TaskQueue.status(server: ctx.queue)
      assert status.in_progress_count == 1
      assert status.queue_size == 0
    end

    test "task_completed message moves to completed", ctx do
      TaskQueue.push_task(%{id: "t1", identifier: "TEZ-300", priority: 1, labels: []},
        server: ctx.queue
      )

      {:ok, _} = TaskQueue.pop_task(server: ctx.queue)

      send(ctx.pid, {:task_completed, "TEZ-300"})
      Process.sleep(50)

      status = TaskQueue.status(server: ctx.queue)
      assert status.completed_count == 1
      assert status.in_progress_count == 0
    end
  end

  describe "status/0" do
    test "returns queue size, in-progress count, completed count", ctx do
      status = TaskQueue.status(server: ctx.queue)

      assert status.queue_size == 0
      assert status.in_progress_count == 0
      assert status.completed_count == 0
    end

    test "reflects queue contents", ctx do
      TaskQueue.push_task(%{id: "t1", identifier: "TEZ-400", priority: 1, labels: []},
        server: ctx.queue
      )

      TaskQueue.push_task(%{id: "t2", identifier: "TEZ-401", priority: 2, labels: []},
        server: ctx.queue
      )

      status = TaskQueue.status(server: ctx.queue)
      assert status.queue_size == 2
    end
  end

  describe "remove_task/1" do
    test "removes a task from the queue", ctx do
      TaskQueue.push_task(%{id: "t1", identifier: "TEZ-500", priority: 1, labels: []},
        server: ctx.queue
      )

      assert :ok = TaskQueue.remove_task("t1", server: ctx.queue)
      assert TaskQueue.status(server: ctx.queue).queue_size == 0
    end

    test "returns error for unknown task", ctx do
      assert {:error, :not_found} = TaskQueue.remove_task("nonexistent", server: ctx.queue)
    end
  end

  describe "list_tasks/0" do
    test "returns all tasks in priority order", ctx do
      TaskQueue.push_task(%{id: "a", identifier: "TEZ-600", priority: 3, labels: []},
        server: ctx.queue
      )

      TaskQueue.push_task(%{id: "b", identifier: "TEZ-601", priority: 1, labels: []},
        server: ctx.queue
      )

      tasks = TaskQueue.list_tasks(server: ctx.queue)
      assert length(tasks) == 2
      assert hd(tasks).priority == 1
    end
  end
end
