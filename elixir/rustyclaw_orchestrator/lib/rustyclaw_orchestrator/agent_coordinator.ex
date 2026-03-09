defmodule RustyclawOrchestrator.AgentCoordinator do
  @moduledoc """
  Capability-based routing and delegation ACL enforcement.

  Routes tasks to agents based on required capabilities and enforces
  per-agent `delegates_to` allowlists from agent definitions.

  Delegation strategies:
  - `:first_available` — route to the first matching agent (default)
  - `:sequential` — try each matching agent in order until one succeeds
  - `:fanout` — send to all matching agents concurrently, collect results
  """

  use GenServer

  alias RustyclawOrchestrator.{AgentServer, AgentSupervisor}

  @call_timeout 30_000

  # --- Client API ---

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Route a task to agents matching the required capabilities.

  Options:
  - `:capabilities` — list of required capabilities (default: [])
  - `:strategy` — `:first_available | :sequential | :fanout` (default: `:first_available`)
  - `:from_agent` — name of the delegating agent (for ACL enforcement)
  """
  @spec delegate(String.t(), keyword()) ::
          {:ok, term()} | {:error, :no_matching_agents | :acl_denied | :all_failed}
  def delegate(task, opts \\ []) do
    GenServer.call(__MODULE__, {:delegate, task, opts}, @call_timeout)
  end

  @doc "Find agents matching the given capabilities."
  @spec find_agents([String.t()]) :: [String.t()]
  def find_agents(capabilities) do
    GenServer.call(__MODULE__, {:find_agents, capabilities}, @call_timeout)
  end

  @doc "Check if agent `from` is allowed to delegate to agent `to`."
  @spec allowed_to_delegate?(String.t(), String.t()) :: boolean()
  def allowed_to_delegate?(from_agent, to_agent) do
    GenServer.call(__MODULE__, {:check_acl, from_agent, to_agent}, @call_timeout)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    {:ok, %{}}
  end

  @impl true
  def handle_call({:delegate, task, opts}, _from, state) do
    capabilities = Keyword.get(opts, :capabilities, [])
    strategy = Keyword.get(opts, :strategy, :first_available)
    from_agent = Keyword.get(opts, :from_agent)

    matching = find_matching_agents(capabilities)

    # Apply ACL filter if delegating from a specific agent
    agents =
      if from_agent do
        Enum.filter(matching, fn name -> check_delegation_acl(from_agent, name) end)
      else
        matching
      end

    result =
      case agents do
        [] when matching != [] and from_agent != nil ->
          {:error, :acl_denied}

        [] ->
          {:error, :no_matching_agents}

        agents ->
          execute_strategy(strategy, task, agents)
      end

    {:reply, result, state}
  end

  def handle_call({:find_agents, capabilities}, _from, state) do
    {:reply, find_matching_agents(capabilities), state}
  end

  def handle_call({:check_acl, from_agent, to_agent}, _from, state) do
    {:reply, check_delegation_acl(from_agent, to_agent), state}
  end

  # --- Internals ---

  defp find_matching_agents(required_capabilities) do
    AgentSupervisor.list_agents()
    |> Enum.filter(fn name ->
      case safe_get_state(name) do
        {:ok, state} ->
          agent_caps = state.definition.capabilities
          Enum.all?(required_capabilities, &(&1 in agent_caps))

        :error ->
          false
      end
    end)
  end

  defp check_delegation_acl(from_agent, to_agent) do
    case safe_get_state(from_agent) do
      {:ok, state} ->
        delegates_to = state.definition.delegates_to
        # Empty delegates_to means agent can delegate to anyone
        delegates_to == [] or to_agent in delegates_to

      :error ->
        false
    end
  end

  defp execute_strategy(:first_available, task, [agent | _]) do
    AgentServer.run_task(agent, task)
  end

  defp execute_strategy(:sequential, task, agents) do
    Enum.reduce_while(agents, {:error, :all_failed}, fn agent, _acc ->
      case AgentServer.run_task(agent, task) do
        {:ok, _} = result -> {:halt, result}
        {:error, _} -> {:cont, {:error, :all_failed}}
      end
    end)
  end

  defp execute_strategy(:fanout, task, agents) do
    results =
      agents
      |> Task.async_stream(fn agent -> {agent, AgentServer.run_task(agent, task)} end,
        timeout: @call_timeout,
        on_timeout: :kill_task
      )
      |> Enum.map(fn
        {:ok, result} -> result
        {:exit, _} -> {:error, :timeout}
      end)

    {:ok, results}
  end

  defp safe_get_state(agent_name) do
    {:ok, AgentServer.get_state(agent_name)}
  catch
    :exit, _ -> :error
  end
end
