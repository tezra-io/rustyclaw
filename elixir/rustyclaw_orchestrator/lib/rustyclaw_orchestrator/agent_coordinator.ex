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

  alias RustyclawOrchestrator.{
    AgentDefinition,
    AgentDiscovery,
    AgentServer,
    AgentSupervisor,
    MessageProvenance
  }

  alias RustyclawOrchestrator.ToolSynthesis.Registry, as: SynthRegistry

  require Logger

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
  - `:provenance` — `MessageProvenance.t()` to propagate through delegation
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

  @doc "Reload agent definitions from disk."
  @spec refresh_definitions() :: :ok
  def refresh_definitions do
    GenServer.call(__MODULE__, :refresh_definitions, @call_timeout)
  end

  @doc "Check if agent `from` is allowed to delegate to agent `to`."
  @spec allowed_to_delegate?(String.t(), String.t()) :: boolean()
  def allowed_to_delegate?(from_agent, to_agent) do
    GenServer.call(__MODULE__, {:check_acl, from_agent, to_agent}, @call_timeout)
  end

  @doc """
  Discover synthesized tools available for agent use.

  Returns tools with `:promoted` or `:probation` status — tools that are
  active and executable. This enables agent-to-agent tool sharing: a tool
  synthesized by one agent is discoverable by all agents.

  Options:
  - `:status` — filter by specific status (default: returns both :promoted and :probation)
  """
  @spec discover_synth_tools(keyword()) :: [map()]
  def discover_synth_tools(opts \\ []) do
    status = Keyword.get(opts, :status)

    tools =
      if status do
        SynthRegistry.list(status: status)
      else
        promoted = SynthRegistry.list(status: :promoted)
        probation = SynthRegistry.list(status: :probation)
        promoted ++ probation
      end

    Enum.map(tools, fn entry ->
      %{
        name: entry.name,
        description: entry.description,
        parameters_schema: entry.parameters_schema,
        status: entry.status,
        author_agent: entry.author_agent,
        invocation_count: entry.invocation_count,
        success_rate: safe_synth_rate(entry)
      }
    end)
  end

  defp safe_synth_rate(%{invocation_count: 0}), do: nil

  defp safe_synth_rate(%{success_count: sc, invocation_count: ic}) do
    sc / ic
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(_opts) do
    definitions = AgentDiscovery.load_definitions()
    {:ok, %{pending: %{}, definitions: definitions}}
  end

  @impl true
  def handle_call({:delegate, task, opts}, from, state) do
    capabilities = Keyword.get(opts, :capabilities, [])
    strategy = Keyword.get(opts, :strategy, :first_available)
    from_agent = Keyword.get(opts, :from_agent)
    provenance = Keyword.get(opts, :provenance)

    matching = find_matching_agents(capabilities, state)

    # Apply ACL filter if delegating from a specific agent
    agents =
      if from_agent do
        Enum.filter(matching, fn name -> check_delegation_acl(from_agent, name) end)
      else
        matching
      end

    case agents do
      [] when matching != [] and from_agent != nil ->
        {:reply, {:error, :acl_denied}, state}

      [] ->
        {:reply, {:error, :no_matching_agents}, state}

      agents ->
        # Dispatch strategy execution to a supervised Task to avoid blocking
        definitions = state.definitions

        %Task{ref: ref} =
          Task.Supervisor.async_nolink(
            __MODULE__.TaskSupervisor,
            fn -> execute_strategy(strategy, task, agents, provenance, definitions) end
          )

        {:noreply, put_in(state, [:pending, ref], from)}
    end
  end

  def handle_call({:find_agents, capabilities}, _from, state) do
    {:reply, find_matching_agents(capabilities, state), state}
  end

  def handle_call({:check_acl, from_agent, to_agent}, _from, state) do
    {:reply, check_delegation_acl(from_agent, to_agent), state}
  end

  def handle_call(:refresh_definitions, _from, state) do
    definitions = AgentDiscovery.load_definitions()
    {:reply, :ok, %{state | definitions: definitions}}
  end

  @impl true
  def handle_info({ref, result}, state) when is_reference(ref) do
    Process.demonitor(ref, [:flush])

    case pop_in(state, [:pending, ref]) do
      {nil, state} ->
        {:noreply, state}

      {from, state} ->
        GenServer.reply(from, result)
        {:noreply, state}
    end
  end

  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    case pop_in(state, [:pending, ref]) do
      {nil, state} ->
        {:noreply, state}

      {from, state} ->
        GenServer.reply(from, {:error, {:task_crashed, reason}})
        {:noreply, state}
    end
  end

  # --- Internals ---

  defp find_matching_agents(required_capabilities, state) do
    # Running agents with their capabilities (preferred — already warm)
    running =
      AgentSupervisor.list_agents()
      |> Enum.flat_map(fn name ->
        case safe_get_state(name) do
          {:ok, agent_state} -> [{name, agent_state.definition.capabilities}]
          :error -> []
        end
      end)

    running_names = MapSet.new(running, fn {name, _} -> name end)

    # Unspawned agents from cached definitions
    unspawned =
      state.definitions
      |> Enum.reject(fn {name, _} -> MapSet.member?(running_names, name) end)
      |> Enum.map(fn {name, def_} -> {name, def_.capabilities} end)

    (running ++ unspawned)
    |> Enum.filter(fn {_name, caps} ->
      Enum.all?(required_capabilities, &(&1 in caps))
    end)
    |> Enum.map(fn {name, _} -> name end)
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

  defp execute_strategy(:first_available, task, [agent | _], provenance, definitions) do
    with :ok <- ensure_agent_running(agent, definitions) do
      child_prov = stamp_provenance(provenance, agent)
      AgentServer.run_task(agent, task, provenance: child_prov)
    end
  end

  defp execute_strategy(:sequential, task, agents, provenance, definitions) do
    Enum.reduce_while(agents, {:error, :all_failed}, fn agent, _acc ->
      sequential_try_agent(agent, task, provenance, definitions)
    end)
  end

  defp execute_strategy(:fanout, task, agents, provenance, definitions) do
    results =
      agents
      |> Task.async_stream(
        fn agent ->
          case ensure_agent_running(agent, definitions) do
            :ok ->
              child_prov = stamp_provenance(provenance, agent)
              {agent, AgentServer.run_task(agent, task, provenance: child_prov)}

            {:error, reason} ->
              {agent, {:error, {:spawn_failed, reason}}}
          end
        end,
        timeout: @call_timeout,
        on_timeout: :kill_task
      )
      |> Enum.map(fn
        {:ok, result} -> result
        {:exit, _} -> {:error, :timeout}
      end)

    {:ok, results}
  end

  defp sequential_try_agent(agent, task, provenance, definitions) do
    case ensure_agent_running(agent, definitions) do
      :ok ->
        child_prov = stamp_provenance(provenance, agent)

        case AgentServer.run_task(agent, task, provenance: child_prov) do
          {:ok, _} = result -> {:halt, result}
          {:error, _} -> {:cont, {:error, :all_failed}}
        end

      {:error, _} ->
        {:cont, {:error, :all_failed}}
    end
  end

  defp ensure_agent_running(agent_name, definitions) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, agent_name) do
      [{_pid, _}] -> :ok
      [] -> spawn_from_definition(agent_name, definitions)
    end
  end

  defp spawn_from_definition(agent_name, definitions) do
    case Map.get(definitions, agent_name) do
      %AgentDefinition{} = def_ ->
        case AgentSupervisor.spawn_agent(def_) do
          {:ok, _pid} -> :ok
          {:error, reason} -> {:error, {:spawn_failed, reason}}
        end

      nil ->
        {:error, :definition_not_found}
    end
  end

  defp safe_get_state(agent_name) do
    {:ok, AgentServer.get_state(agent_name)}
  catch
    :exit, _ -> :error
  end

  defp stamp_provenance(%MessageProvenance{} = prov, target_agent) do
    child = MessageProvenance.propagate(prov, source_agent: target_agent)

    Logger.info("Delegation routing",
      trace_id: child.trace_id,
      from: prov.source_agent,
      to: target_agent,
      delegation_depth: child.delegation_depth
    )

    child
  end

  defp stamp_provenance(nil, _target_agent), do: nil
end
