defmodule RustyclawOrchestrator.Plugins.ManagerTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.Manager

  defmodule HealthyPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, %{config: config}}

    @impl true
    def execute(state, _task, _handler), do: {:ok, {:complete, :done}, state}

    @impl true
    def health(_state), do: :healthy

    @impl true
    def capabilities, do: [:coding]

    @impl true
    def rate_limit_status(_state), do: %{remaining: 50, reset_at: nil, limited: false}

    @impl true
    def disconnect(_state), do: :ok
  end

  defmodule ReviewPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(config), do: {:ok, %{config: config}}

    @impl true
    def execute(state, _task, _handler), do: {:ok, {:complete, :reviewed}, state}

    @impl true
    def health(_state), do: :healthy

    @impl true
    def capabilities, do: [:review, :coding]

    @impl true
    def rate_limit_status(_state), do: %{remaining: 10, reset_at: nil, limited: false}

    @impl true
    def disconnect(_state), do: :ok
  end

  defmodule FailPlugin do
    @behaviour RustyclawOrchestrator.Plugins.Behaviour

    @impl true
    def connect(_config), do: {:error, :connection_refused}

    @impl true
    def execute(state, _task, _handler), do: {:ok, {:complete, :done}, state}

    @impl true
    def health(_state), do: :unhealthy

    @impl true
    def capabilities, do: [:coding]

    @impl true
    def rate_limit_status(_state), do: %{remaining: 0, reset_at: nil, limited: true}

    @impl true
    def disconnect(_state), do: :ok
  end

  setup do
    server = :"manager_test_#{:erlang.unique_integer([:positive])}"
    start_supervised!({Manager, name: server})
    %{server: server}
  end

  describe "start_plugin/2" do
    test "starts a plugin successfully", %{server: server} do
      config = %{name: "test_plugin", module: HealthyPlugin, config: %{api_key: "key"}}
      assert {:ok, entry} = Manager.start_plugin(config, server: server)
      assert entry.name == "test_plugin"
      assert entry.status == :healthy
      assert entry.capabilities == [:coding]
    end

    test "returns error for duplicate plugin name", %{server: server} do
      config = %{name: "dupe", module: HealthyPlugin}
      assert {:ok, _} = Manager.start_plugin(config, server: server)
      assert {:error, :already_started} = Manager.start_plugin(config, server: server)
    end

    test "returns error when connect fails", %{server: server} do
      config = %{name: "broken", module: FailPlugin}
      assert {:error, :connection_refused} = Manager.start_plugin(config, server: server)
    end
  end

  describe "stop_plugin/2" do
    test "stops a running plugin", %{server: server} do
      config = %{name: "to_stop", module: HealthyPlugin}
      {:ok, _} = Manager.start_plugin(config, server: server)
      assert :ok = Manager.stop_plugin("to_stop", server: server)
      assert {:error, :not_found} = Manager.get_plugin("to_stop", server: server)
    end

    test "returns error for unknown plugin", %{server: server} do
      assert {:error, :not_found} = Manager.stop_plugin("nonexistent", server: server)
    end
  end

  describe "plugins_for_capabilities/2" do
    test "returns plugins matching capabilities", %{server: server} do
      Manager.start_plugin(%{name: "coder", module: HealthyPlugin}, server: server)
      Manager.start_plugin(%{name: "reviewer", module: ReviewPlugin}, server: server)

      matches = Manager.plugins_for_capabilities([:coding], server: server)
      names = Enum.map(matches, & &1.name)
      assert "coder" in names
      assert "reviewer" in names
    end

    test "filters by specific capability", %{server: server} do
      Manager.start_plugin(%{name: "coder", module: HealthyPlugin}, server: server)
      Manager.start_plugin(%{name: "reviewer", module: ReviewPlugin}, server: server)

      matches = Manager.plugins_for_capabilities([:review], server: server)
      names = Enum.map(matches, & &1.name)
      refute "coder" in names
      assert "reviewer" in names
    end

    test "excludes rate-limited plugins", %{server: server} do
      Manager.start_plugin(%{name: "limited", module: HealthyPlugin}, server: server)
      Manager.report_rate_limit("limited", 60, server: server)
      # Give the cast time to process
      Process.sleep(50)

      matches = Manager.plugins_for_capabilities([:coding], server: server)
      names = Enum.map(matches, & &1.name)
      refute "limited" in names
    end

    test "returns empty list when no match", %{server: server} do
      Manager.start_plugin(%{name: "coder", module: HealthyPlugin}, server: server)
      assert [] = Manager.plugins_for_capabilities([:research], server: server)
    end
  end

  describe "report_rate_limit/3" do
    test "marks plugin as rate limited", %{server: server} do
      Manager.start_plugin(%{name: "rl_test", module: HealthyPlugin}, server: server)
      Manager.report_rate_limit("rl_test", 5, server: server)
      Process.sleep(50)

      {:ok, entry} = Manager.get_plugin("rl_test", server: server)
      assert entry.status == :rate_limited
      assert entry.rate_limit.limited == true
      assert entry.rate_limit.remaining == 0
      assert %DateTime{} = entry.rate_limit.reset_at
    end

    test "clears rate limit after timeout", %{server: server} do
      Manager.start_plugin(%{name: "rl_clear", module: HealthyPlugin}, server: server)
      # Use 1-second timeout for fast test
      Manager.report_rate_limit("rl_clear", 1, server: server)
      Process.sleep(50)

      {:ok, entry} = Manager.get_plugin("rl_clear", server: server)
      assert entry.rate_limit.limited == true

      # Wait for clear
      Process.sleep(1_100)

      {:ok, entry} = Manager.get_plugin("rl_clear", server: server)
      assert entry.rate_limit.limited == false
      assert entry.status == :healthy
    end
  end

  describe "list_plugins/1" do
    test "lists all registered plugins", %{server: server} do
      Manager.start_plugin(%{name: "p1", module: HealthyPlugin}, server: server)
      Manager.start_plugin(%{name: "p2", module: ReviewPlugin}, server: server)

      plugins = Manager.list_plugins(server: server)
      assert length(plugins) == 2
      names = Enum.map(plugins, & &1.name)
      assert "p1" in names
      assert "p2" in names
    end

    test "returns empty list when no plugins", %{server: server} do
      assert [] = Manager.list_plugins(server: server)
    end
  end

  describe "get_plugin/2" do
    test "returns plugin entry", %{server: server} do
      Manager.start_plugin(%{name: "get_me", module: HealthyPlugin}, server: server)
      assert {:ok, entry} = Manager.get_plugin("get_me", server: server)
      assert entry.name == "get_me"
    end

    test "returns error for missing plugin", %{server: server} do
      assert {:error, :not_found} = Manager.get_plugin("ghost", server: server)
    end
  end
end
