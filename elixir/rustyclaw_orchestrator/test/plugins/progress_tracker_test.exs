defmodule RustyclawOrchestrator.Plugins.ProgressTrackerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.ProgressTracker

  setup do
    name = :"tracker_test_#{:erlang.unique_integer([:positive])}"

    tracker =
      start_supervised!(
        {ProgressTracker,
         name: name, window_size: 5, similarity_threshold: 0.85, stuck_timeout_ms: 300_000}
      )

    %{tracker: tracker}
  end

  describe "record/3 and get_worker_state/2" do
    test "tracks events for a worker", %{tracker: tracker} do
      ProgressTracker.record(tracker, "w1", {:chunk, "hello"})
      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.event_count == 1
      assert ws.last_event_at != nil
    end

    test "increments event count", %{tracker: tracker} do
      ProgressTracker.record(tracker, "w1", {:chunk, "a"})
      ProgressTracker.record(tracker, "w1", {:chunk, "b"})
      ProgressTracker.record(tracker, "w1", {:chunk, "c"})
      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.event_count == 3
    end

    test "tracks multiple workers independently", %{tracker: tracker} do
      ProgressTracker.record(tracker, "w1", {:chunk, "a"})
      ProgressTracker.record(tracker, "w2", {:chunk, "b"})
      Process.sleep(50)

      assert {:ok, ws1} = ProgressTracker.get_worker_state(tracker, "w1")
      assert {:ok, ws2} = ProgressTracker.get_worker_state(tracker, "w2")
      assert ws1.event_count == 1
      assert ws2.event_count == 1
    end

    test "returns not_found for unknown worker", %{tracker: tracker} do
      assert {:error, :not_found} = ProgressTracker.get_worker_state(tracker, "unknown")
    end
  end

  describe "artifact tracking" do
    test "tracks artifacts in bounded queue", %{tracker: tracker} do
      for i <- 1..7 do
        ProgressTracker.record(tracker, "w1", {:artifact, :code, "version #{i}"})
      end

      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      # Queue bounded to window_size=5
      artifacts = :queue.to_list(ws.recent_artifacts)
      assert length(artifacts) == 5
      # Should have versions 3-7 (oldest dropped)
      assert Enum.at(artifacts, 0).content == "version 3"
      assert Enum.at(artifacts, 4).content == "version 7"
    end
  end

  describe "loop detection" do
    test "detects loop after 3 consecutive similar artifacts", %{tracker: tracker} do
      # Send 4 nearly identical artifacts to trigger loop (need 3 consecutive similar checks)
      for _i <- 1..4 do
        ProgressTracker.record(tracker, "w1", {:artifact, :code, "function foo() { return 42; }"})
      end

      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.loop_detected == true
      assert ws.consecutive_similar >= 3
    end

    test "does not trigger loop for different artifacts" do
      name = :"tracker_no_loop_#{:erlang.unique_integer([:positive])}"

      tracker =
        start_supervised!(
          {ProgressTracker,
           name: name, window_size: 5, similarity_threshold: 0.85, stuck_timeout_ms: 300_000},
          id: name
        )

      ProgressTracker.record(tracker, "w1", {:artifact, :code, "function alpha() {}"})
      ProgressTracker.record(tracker, "w1", {:artifact, :code, "function beta() {}"})
      ProgressTracker.record(tracker, "w1", {:artifact, :code, "function gamma() {}"})
      ProgressTracker.record(tracker, "w1", {:artifact, :code, "class MyWidget {}"})
      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.loop_detected == false
    end

    test "detects loop for very similar but not identical artifacts" do
      name = :"tracker_similar_#{:erlang.unique_integer([:positive])}"

      tracker =
        start_supervised!(
          {ProgressTracker,
           name: name, window_size: 5, similarity_threshold: 0.85, stuck_timeout_ms: 300_000},
          id: name
        )

      # These are >85% similar (only differ by a small number)
      for i <- 1..4 do
        ProgressTracker.record(
          tracker,
          "w1",
          {:artifact, :code,
           "function calculateTotal(items) { return items.reduce((sum, item) => sum + item.price, #{i}); }"}
        )
      end

      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.loop_detected == true
    end

    test "resets consecutive count when different artifact appears" do
      name = :"tracker_reset_#{:erlang.unique_integer([:positive])}"

      tracker =
        start_supervised!(
          {ProgressTracker,
           name: name, window_size: 5, similarity_threshold: 0.85, stuck_timeout_ms: 300_000},
          id: name
        )

      # Two similar
      ProgressTracker.record(tracker, "w1", {:artifact, :code, "function foo() { return 1; }"})
      ProgressTracker.record(tracker, "w1", {:artifact, :code, "function foo() { return 2; }"})
      # One very different — breaks the streak
      ProgressTracker.record(
        tracker,
        "w1",
        {:artifact, :code, "class CompletelyDifferentThing { constructor() { this.x = 999; } }"}
      )

      Process.sleep(50)

      assert {:ok, ws} = ProgressTracker.get_worker_state(tracker, "w1")
      assert ws.loop_detected == false
      assert ws.consecutive_similar == 0
    end

    test "calls on_loop_detected callback" do
      test_pid = self()
      ref = make_ref()

      name = :"tracker_callback_#{:erlang.unique_integer([:positive])}"

      tracker =
        start_supervised!(
          {ProgressTracker,
           name: name,
           window_size: 5,
           similarity_threshold: 0.85,
           stuck_timeout_ms: 300_000,
           on_loop_detected: fn type, _content -> send(test_pid, {ref, :loop, type}) end},
          id: name
        )

      for _i <- 1..4 do
        ProgressTracker.record(tracker, "w1", {:artifact, :code, "repeated content here"})
      end

      Process.sleep(50)

      assert_received {^ref, :loop, :code}
    end
  end

  describe "stuck detection" do
    test "calls on_stuck_detected callback when worker is inactive" do
      test_pid = self()
      ref = make_ref()

      name = :"tracker_stuck_#{:erlang.unique_integer([:positive])}"

      # Use a very short stuck timeout for testing
      tracker =
        start_supervised!(
          {ProgressTracker,
           name: name,
           window_size: 5,
           similarity_threshold: 0.85,
           stuck_timeout_ms: 100,
           on_stuck_detected: fn worker_id -> send(test_pid, {ref, :stuck, worker_id}) end},
          id: name
        )

      # Record one event, then go silent
      ProgressTracker.record(tracker, "w1", {:chunk, "started"})

      # Wait for the stuck check to fire
      assert_receive {^ref, :stuck, "w1"}, 2_000
    end
  end

  describe "clear_worker/2" do
    test "removes worker state", %{tracker: tracker} do
      ProgressTracker.record(tracker, "w1", {:chunk, "hello"})
      Process.sleep(50)

      assert {:ok, _} = ProgressTracker.get_worker_state(tracker, "w1")

      ProgressTracker.clear_worker(tracker, "w1")
      Process.sleep(50)

      assert {:error, :not_found} = ProgressTracker.get_worker_state(tracker, "w1")
    end
  end

  describe "list_workers/1" do
    test "lists tracked workers", %{tracker: tracker} do
      ProgressTracker.record(tracker, "w1", {:chunk, "a"})
      ProgressTracker.record(tracker, "w2", {:chunk, "b"})
      Process.sleep(50)

      workers = ProgressTracker.list_workers(tracker)
      assert "w1" in workers
      assert "w2" in workers
    end

    test "returns empty list when no workers", %{tracker: tracker} do
      assert [] = ProgressTracker.list_workers(tracker)
    end
  end

  describe "levenshtein_similarity/2" do
    test "identical strings have similarity 1.0" do
      assert ProgressTracker.levenshtein_similarity("hello", "hello") == 1.0
    end

    test "completely different strings have low similarity" do
      sim = ProgressTracker.levenshtein_similarity("abc", "xyz")
      assert sim < 0.5
    end

    test "empty strings have similarity 1.0" do
      assert ProgressTracker.levenshtein_similarity("", "") == 1.0
    end

    test "one empty string has similarity 0.0" do
      assert ProgressTracker.levenshtein_similarity("hello", "") == 0.0
    end

    test "similar strings have high similarity" do
      sim =
        ProgressTracker.levenshtein_similarity(
          "function foo() { return 1; }",
          "function foo() { return 2; }"
        )

      assert sim > 0.9
    end
  end

  describe "levenshtein_distance/2" do
    test "identical strings have distance 0" do
      assert ProgressTracker.levenshtein_distance("hello", "hello") == 0
    end

    test "single char difference" do
      assert ProgressTracker.levenshtein_distance("cat", "bat") == 1
    end

    test "insertion" do
      assert ProgressTracker.levenshtein_distance("cat", "cats") == 1
    end

    test "deletion" do
      assert ProgressTracker.levenshtein_distance("cats", "cat") == 1
    end

    test "empty to non-empty" do
      assert ProgressTracker.levenshtein_distance("", "abc") == 3
    end

    test "non-empty to empty" do
      assert ProgressTracker.levenshtein_distance("abc", "") == 3
    end

    test "both empty" do
      assert ProgressTracker.levenshtein_distance("", "") == 0
    end

    test "kitten to sitting" do
      assert ProgressTracker.levenshtein_distance("kitten", "sitting") == 3
    end
  end
end
