defmodule RustyclawOrchestrator.Tools.SpawnAgentTool do
  @moduledoc """
  Tool for dynamically spawning sub-agents at runtime.

  Accepts a map with agent configuration and spawns a new supervised agent.
  Validates all inputs before spawning. Establishes parent-child relationships
  when a parent agent name is provided.

  ## Parameters

    * `name` — required, unique agent name (string)
    * `capabilities` — list of capability strings (default: [])
    * `persistent` — whether to survive restarts (default: false)
    * `parent` — parent agent name for hierarchy (optional)
    * `max_memory_mb` — memory limit in MB (optional)
    * `model` — LLM model identifier (optional)
    * `personality` — system prompt / personality text (default: "")
    * `delegates_to` — list of agent names this agent can delegate to (default: [])

  ## Returns

    * `{:ok, %{agent_name: String.t(), pid: pid()}}` on success
    * `{:error, reason}` on failure
  """

  alias RustyclawOrchestrator.{AgentDefinition, AgentSupervisor}

  @required_params [:name]

  @doc "Execute the spawn_agent tool."
  @spec execute(map()) :: {:ok, map()} | {:error, String.t()}
  def execute(params) when is_map(params) do
    with {:ok, validated} <- validate_params(params),
         {:ok, definition} <- build_definition(validated),
         {:ok, pid} <- do_spawn(definition, validated) do
      {:ok, %{agent_name: definition.name, pid: pid}}
    end
  end

  def execute(_), do: {:error, "params must be a map"}

  defp validate_params(params) do
    missing =
      @required_params
      |> Enum.reject(fn key ->
        Map.has_key?(params, key) or Map.has_key?(params, to_string(key))
      end)

    case missing do
      [] -> {:ok, normalize_keys(params)}
      keys -> {:error, "missing required parameters: #{Enum.join(keys, ", ")}"}
    end
  end

  defp normalize_keys(params) do
    Map.new(params, fn
      {k, v} when is_binary(k) -> {String.to_existing_atom(k), v}
      {k, v} when is_atom(k) -> {k, v}
    end)
  rescue
    ArgumentError -> params
  end

  defp build_definition(params) do
    name = params[:name] || params["name"]

    with :ok <- validate_name(name) do
      {:ok,
       %AgentDefinition{
         name: name,
         capabilities: params[:capabilities] || [],
         persistent: params[:persistent] || false,
         parent: params[:parent],
         max_memory_mb: params[:max_memory_mb],
         model: params[:model],
         personality: params[:personality] || "",
         delegates_to: params[:delegates_to] || []
       }}
    end
  end

  defp validate_name(name) when not is_binary(name), do: {:error, "name must be a string"}
  defp validate_name(""), do: {:error, "name cannot be empty"}

  defp validate_name(name) do
    if String.contains?(name, ["/", "\\"]),
      do: {:error, "name cannot contain path separators"},
      else: :ok
  end

  defp do_spawn(definition, params) do
    parent_pid = resolve_parent_pid(params[:parent])
    AgentSupervisor.spawn_agent(definition, parent_pid: parent_pid)
  end

  defp resolve_parent_pid(nil), do: nil

  defp resolve_parent_pid(parent_name) when is_binary(parent_name) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, parent_name) do
      [{pid, _}] -> pid
      [] -> nil
    end
  end
end
