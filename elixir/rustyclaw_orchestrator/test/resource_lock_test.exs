defmodule RustyclawOrchestrator.ResourceLockTest do
  @moduledoc """
  Tests for ResourceLock: acquisition, release, contention, and dead-process cleanup.
  """

  use ExUnit.Case

  alias RustyclawOrchestrator.ResourceLock

  setup do
    # Clean up any leftover locks between tests
    on_exit(fn ->
      for resource <- [
            "test-res",
            "browser",
            "serial",
            "dead-res",
            "contend-res",
            "multi-res",
            "stale-mon"
          ] do
        ResourceLock.release(resource)
        # Also force-clean ETS in case release fails (different process)
        try do
          :ets.delete(ResourceLock, resource)
        rescue
          ArgumentError -> :ok
        end
      end
    end)

    :ok
  end

  # --- Basic acquire/release ---

  describe "acquire/release" do
    test "acquire and release a resource" do
      assert :ok = ResourceLock.acquire("test-res", wait_ms: 100)
      assert ResourceLock.locked?("test-res")
      assert :ok = ResourceLock.release("test-res")
      refute ResourceLock.locked?("test-res")
    end

    test "double acquire by same process fails" do
      assert :ok = ResourceLock.acquire("test-res", wait_ms: 100)
      # Same process tries again — ETS insert_new will fail, but we hold it
      # so it spins until timeout
      assert {:error, :resource_busy} = ResourceLock.acquire("test-res", wait_ms: 100)
      ResourceLock.release("test-res")
    end

    test "release by non-holder returns error" do
      assert {:error, :not_held} = ResourceLock.release("test-res")
    end
  end

  # --- Holder tracking ---

  describe "holder/1" do
    test "returns pid of holder" do
      :ok = ResourceLock.acquire("test-res", wait_ms: 100)
      assert ResourceLock.holder("test-res") == self()
      ResourceLock.release("test-res")
    end

    test "returns nil for unlocked resource" do
      assert ResourceLock.holder("test-res") == nil
    end
  end

  # --- Cross-process contention ---

  describe "contention" do
    test "second process waits and acquires after release" do
      test_pid = self()

      # First process acquires
      :ok = ResourceLock.acquire("contend-res", wait_ms: 1_000)

      # Second process tries to acquire — will block
      task =
        Task.async(fn ->
          result = ResourceLock.acquire("contend-res", wait_ms: 2_000)
          send(test_pid, {:acquired, result})
          ResourceLock.release("contend-res")
        end)

      # Release after a short delay
      Process.sleep(100)
      ResourceLock.release("contend-res")

      # Second process should have acquired
      assert_receive {:acquired, :ok}, 3_000
      Task.await(task, 5_000)
    end

    test "second process times out if resource held too long" do
      test_pid = self()

      :ok = ResourceLock.acquire("contend-res", wait_ms: 5_000)

      task =
        Task.async(fn ->
          result = ResourceLock.acquire("contend-res", wait_ms: 200)
          send(test_pid, {:result, result})
        end)

      assert_receive {:result, {:error, :resource_busy}}, 2_000
      ResourceLock.release("contend-res")
      Task.await(task, 5_000)
    end
  end

  # --- Dead process cleanup ---

  describe "dead process cleanup" do
    test "lock is reclaimed when holder dies" do
      # Spawn a process that acquires and then dies
      {pid, ref} =
        spawn_monitor(fn ->
          ResourceLock.acquire("dead-res", wait_ms: 100)
          # Process exits normally, monitor process cleans up
        end)

      receive do
        {:DOWN, ^ref, :process, ^pid, :normal} -> :ok
      after
        1_000 -> flunk("Spawned process did not exit")
      end

      # Give the monitor process time to clean up
      Process.sleep(50)

      # Lock should be reclaimable
      assert :ok = ResourceLock.acquire("dead-res", wait_ms: 500)
      ResourceLock.release("dead-res")
    end
  end

  # --- Stale monitor safety ---

  describe "stale monitor" do
    test "prior holder's monitor does not delete new holder's lock" do
      test_pid = self()

      # Holder A acquires, then releases, stays alive until told to die
      holder_a =
        spawn(fn ->
          :ok = ResourceLock.acquire("stale-mon", wait_ms: 100)
          send(test_pid, :a_acquired)

          receive do
            :release -> :ok
          end

          ResourceLock.release("stale-mon")
          send(test_pid, :a_released)

          receive do
            :die -> :ok
          end
        end)

      assert_receive :a_acquired, 1_000

      # A releases the lock
      send(holder_a, :release)
      assert_receive :a_released, 1_000

      # Holder B acquires the same resource
      holder_b =
        spawn(fn ->
          :ok = ResourceLock.acquire("stale-mon", wait_ms: 500)
          send(test_pid, :b_acquired)

          receive do
            :done -> :ok
          end
        end)

      assert_receive :b_acquired, 1_000
      assert ResourceLock.holder("stale-mon") == holder_b

      # Kill A — its stale monitor fires
      Process.exit(holder_a, :kill)
      Process.sleep(100)

      # B's lock must still be intact
      assert ResourceLock.locked?("stale-mon")
      assert ResourceLock.holder("stale-mon") == holder_b

      send(holder_b, :done)
    end
  end

  # --- locked?/1 ---

  describe "locked?/1" do
    test "returns false for never-locked resource" do
      refute ResourceLock.locked?("nonexistent")
    end

    test "returns true while held" do
      :ok = ResourceLock.acquire("test-res", wait_ms: 100)
      assert ResourceLock.locked?("test-res")
      ResourceLock.release("test-res")
    end

    test "returns false after release" do
      :ok = ResourceLock.acquire("test-res", wait_ms: 100)
      ResourceLock.release("test-res")
      refute ResourceLock.locked?("test-res")
    end
  end
end
