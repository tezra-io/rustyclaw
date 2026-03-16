defmodule RustyclawOrchestrator.Plugins.BehaviourTest do
  use ExUnit.Case, async: true

  defmodule MockPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(%{should_fail: true}), do: {:error, :connection_failed}

    def connect(config) do
      {:ok, %{connected: true, config: config}}
    end

    @impl true
    def execute(state, task, event_handler) do
      event_handler.({:chunk, "processing"})

      case Map.get(task, :action) do
        :complete ->
          {:ok, {:complete, %{result: "done"}}, state}

        :tool_use ->
          {:ok, {:tool_use, [%{name: "shell", args: %{cmd: "echo hi"}}]}, state}

        :fail ->
          {:error, :task_failed}

        _ ->
          {:ok, {:complete, %{result: "default"}}, state}
      end
    end

    @impl true
    def health(_state), do: :healthy

    @impl true
    def capabilities, do: [:coding, :testing]

    @impl true
    def rate_limit_status(_state) do
      %{remaining: 100, reset_at: nil, limited: false}
    end

    @impl true
    def disconnect(_state), do: :ok
  end

  describe "behaviour contract" do
    test "connect/1 returns {:ok, state}" do
      assert {:ok, state} = MockPlugin.connect(%{api_key: "test"})
      assert state.connected == true
    end

    test "connect/1 returns {:error, reason} on failure" do
      assert {:error, :connection_failed} = MockPlugin.connect(%{should_fail: true})
    end

    test "execute/3 returns {:ok, {:complete, result}, state}" do
      {:ok, state} = MockPlugin.connect(%{})
      handler = fn _event -> :ok end

      assert {:ok, {:complete, %{result: "done"}}, ^state} =
               MockPlugin.execute(state, %{action: :complete}, handler)
    end

    test "execute/3 returns {:ok, {:tool_use, calls}, state}" do
      {:ok, state} = MockPlugin.connect(%{})
      handler = fn _event -> :ok end

      assert {:ok, {:tool_use, [%{name: "shell"}]}, ^state} =
               MockPlugin.execute(state, %{action: :tool_use}, handler)
    end

    test "execute/3 returns {:error, reason}" do
      {:ok, state} = MockPlugin.connect(%{})
      handler = fn _event -> :ok end

      assert {:error, :task_failed} = MockPlugin.execute(state, %{action: :fail}, handler)
    end

    test "execute/3 calls event_handler" do
      {:ok, state} = MockPlugin.connect(%{})
      test_pid = self()
      handler = fn event -> send(test_pid, {:event, event}) end

      MockPlugin.execute(state, %{action: :complete}, handler)

      assert_received {:event, {:chunk, "processing"}}
    end

    test "health/1 returns health status" do
      {:ok, state} = MockPlugin.connect(%{})
      assert :healthy = MockPlugin.health(state)
    end

    test "capabilities/0 returns list of atoms" do
      caps = MockPlugin.capabilities()
      assert is_list(caps)
      assert :coding in caps
      assert :testing in caps
    end

    test "rate_limit_status/1 returns rate limit map" do
      {:ok, state} = MockPlugin.connect(%{})
      status = MockPlugin.rate_limit_status(state)

      assert is_map(status)
      assert Map.has_key?(status, :remaining)
      assert Map.has_key?(status, :reset_at)
      assert Map.has_key?(status, :limited)
      assert status.limited == false
    end

    test "disconnect/1 returns :ok" do
      {:ok, state} = MockPlugin.connect(%{})
      assert :ok = MockPlugin.disconnect(state)
    end
  end
end
