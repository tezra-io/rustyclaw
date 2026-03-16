defmodule RustyclawOrchestrator.Plugins.Manager do
  @moduledoc """
  GenServer managing the plugin pool.

  Handles plugin lifecycle (start/stop), rate limit tracking,
  health monitoring, and capability-based routing.
  """

  use GenServer

  require Logger

  @call_timeout 30_000

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, _init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, opts, name: name)
  end

  @doc "Start a plugin and add it to the pool."
  @spec start_plugin(map(), keyword()) :: {:ok, term()} | {:error, term()}
  def start_plugin(plugin_config, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:start_plugin, plugin_config}, @call_timeout)
  end

  @doc "Stop a plugin and remove it from the pool."
  @spec stop_plugin(String.t(), keyword()) :: :ok | {:error, term()}
  def stop_plugin(plugin_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:stop_plugin, plugin_name}, @call_timeout)
  end

  @doc "Find plugins matching the given capabilities."
  @spec plugins_for_capabilities([atom()], keyword()) :: [map()]
  def plugins_for_capabilities(capabilities, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:plugins_for_capabilities, capabilities}, @call_timeout)
  end

  @doc "Report a rate limit hit for a plugin."
  @spec report_rate_limit(String.t(), non_neg_integer(), keyword()) :: :ok
  def report_rate_limit(plugin_name, retry_after_seconds, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.cast(server, {:report_rate_limit, plugin_name, retry_after_seconds})
  end

  @doc "Get the current state of a plugin."
  @spec get_plugin(String.t(), keyword()) :: {:ok, map()} | {:error, :not_found}
  def get_plugin(plugin_name, opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, {:get_plugin, plugin_name}, @call_timeout)
  end

  @doc "List all registered plugins."
  @spec list_plugins(keyword()) :: [map()]
  def list_plugins(opts \\ []) do
    server = Keyword.get(opts, :server, __MODULE__)
    GenServer.call(server, :list_plugins, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    {:ok, %{plugins: %{}}}
  end

  @impl true
  def handle_call({:start_plugin, config}, _from, state) do
    name = config[:name] || config["name"]
    module = config[:module] || config["module"]

    if Map.has_key?(state.plugins, name) do
      {:reply, {:error, :already_started}, state}
    else
      connect_config = config[:config] || config["config"] || config

      case module.connect(connect_config) do
        {:ok, plugin_state} ->
          entry = %{
            name: name,
            module: module,
            state: plugin_state,
            status: :healthy,
            capabilities: module.capabilities(),
            rate_limit: %{remaining: nil, reset_at: nil, limited: false},
            started_at: DateTime.utc_now()
          }

          plugins = Map.put(state.plugins, name, entry)
          {:reply, {:ok, entry}, %{state | plugins: plugins}}

        {:error, reason} ->
          {:reply, {:error, reason}, state}
      end
    end
  end

  def handle_call({:stop_plugin, name}, _from, state) do
    case Map.pop(state.plugins, name) do
      {nil, _} ->
        {:reply, {:error, :not_found}, state}

      {entry, plugins} ->
        entry.module.disconnect(entry.state)
        {:reply, :ok, %{state | plugins: plugins}}
    end
  end

  def handle_call({:plugins_for_capabilities, requested}, _from, state) do
    matches =
      state.plugins
      |> Map.values()
      |> Enum.filter(fn entry ->
        entry.status != :unhealthy and
          not entry.rate_limit.limited and
          Enum.any?(requested, &(&1 in entry.capabilities))
      end)
      |> Enum.map(fn entry ->
        %{
          name: entry.name,
          module: entry.module,
          state: entry.state,
          capabilities: entry.capabilities,
          status: entry.status
        }
      end)

    {:reply, matches, state}
  end

  def handle_call({:get_plugin, name}, _from, state) do
    case Map.get(state.plugins, name) do
      nil -> {:reply, {:error, :not_found}, state}
      entry -> {:reply, {:ok, entry}, state}
    end
  end

  def handle_call(:list_plugins, _from, state) do
    plugins =
      state.plugins
      |> Map.values()
      |> Enum.map(fn entry ->
        %{
          name: entry.name,
          module: entry.module,
          status: entry.status,
          capabilities: entry.capabilities,
          rate_limit: entry.rate_limit
        }
      end)

    {:reply, plugins, state}
  end

  @impl true
  def handle_cast({:report_rate_limit, name, retry_after_seconds}, state) do
    case Map.get(state.plugins, name) do
      nil ->
        {:noreply, state}

      entry ->
        reset_at = DateTime.add(DateTime.utc_now(), retry_after_seconds, :second)

        updated_entry = %{
          entry
          | status: :rate_limited,
            rate_limit: %{remaining: 0, reset_at: reset_at, limited: true}
        }

        plugins = Map.put(state.plugins, name, updated_entry)

        # Schedule rate limit clear
        Process.send_after(self(), {:clear_rate_limit, name}, retry_after_seconds * 1_000)

        Logger.info("Plugin #{name} rate limited until #{DateTime.to_iso8601(reset_at)}")
        {:noreply, %{state | plugins: plugins}}
    end
  end

  @impl true
  def handle_info({:clear_rate_limit, name}, state) do
    case Map.get(state.plugins, name) do
      nil ->
        {:noreply, state}

      entry ->
        # Check health before restoring
        health = entry.module.health(entry.state)

        updated_entry = %{
          entry
          | status: health,
            rate_limit: %{remaining: nil, reset_at: nil, limited: false}
        }

        plugins = Map.put(state.plugins, name, updated_entry)
        Logger.info("Plugin #{name} rate limit cleared, status: #{health}")
        {:noreply, %{state | plugins: plugins}}
    end
  end
end
