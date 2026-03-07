defmodule ZeroclawOrchestrator.AgentDefinition do
  @moduledoc """
  Parses and validates agent definition files (YAML frontmatter + markdown body).

  Agent definitions live at `~/.zeroclaw/agents/<name>.md` with the format:

      ---
      name: my-agent
      persistent: true
      capabilities:
        - web_search
      ---

      You are an agent that searches the web.

  """

  @type memory_isolation :: :isolated | :shared_read | :shared

  @type t :: %__MODULE__{
          name: String.t(),
          persistent: boolean(),
          skills: [String.t()],
          memory: memory_isolation(),
          memory_backend: String.t(),
          schedule: String.t() | nil,
          channels: [String.t()],
          delegates_to: [String.t()],
          model: String.t() | nil,
          temperature: float() | nil,
          max_tools_per_turn: pos_integer(),
          allowed_tools: [String.t()],
          capabilities: [String.t()],
          personality: String.t()
        }

  @enforce_keys [:name]
  defstruct name: nil,
            persistent: false,
            skills: [],
            memory: :isolated,
            memory_backend: "markdown",
            schedule: nil,
            channels: [],
            delegates_to: [],
            model: nil,
            temperature: nil,
            max_tools_per_turn: 10,
            allowed_tools: [],
            capabilities: [],
            personality: ""

  @valid_memory_isolations ~w(isolated shared-read shared)
  @valid_memory_backends ~w(markdown sqlite lucid none)

  @nimble_schema NimbleOptions.new!([
    name: [type: :string, required: true],
    persistent: [type: :boolean, default: false],
    skills: [type: {:list, :string}, default: []],
    memory: [type: :string, default: "isolated"],
    memory_backend: [type: :string, default: "markdown"],
    schedule: [type: {:or, [:string, nil]}, default: nil],
    channels: [type: {:list, :string}, default: []],
    delegates_to: [type: {:list, :string}, default: []],
    model: [type: {:or, [:string, nil]}, default: nil],
    temperature: [type: {:or, [:float, :integer, nil]}, default: nil],
    max_tools_per_turn: [type: :pos_integer, default: 10],
    allowed_tools: [type: {:list, :string}, default: []],
    capabilities: [type: {:list, :string}, default: []]
  ])

  @doc """
  Parse an agent definition from a markdown string with YAML frontmatter.
  Returns `{:ok, definition}` or `{:error, reason}`.
  """
  @spec parse(String.t()) :: {:ok, t()} | {:error, String.t()}
  def parse(content) do
    with {:ok, yaml_str, body} <- extract_frontmatter(content),
         {:ok, yaml_map} <- parse_yaml(yaml_str),
         {:ok, opts} <- validate_schema(yaml_map),
         {:ok, definition} <- build_definition(opts, body) do
      {:ok, definition}
    end
  end

  @doc """
  Parse an agent definition from a file path.
  """
  @spec from_file(Path.t()) :: {:ok, t()} | {:error, String.t()}
  def from_file(path) do
    case File.read(path) do
      {:ok, content} -> parse(content)
      {:error, reason} -> {:error, "Failed to read #{path}: #{reason}"}
    end
  end

  @doc """
  Validate a parsed definition. Returns `{:ok, warnings}` or `{:error, reason}`.
  Warnings are non-fatal issues (e.g., unknown skill, non-snake_case capability).
  """
  @spec validate(t(), [String.t()]) :: {:ok, [String.t()]} | {:error, String.t()}
  def validate(%__MODULE__{} = def, available_skills \\ []) do
    with :ok <- validate_name(def.name),
         :ok <- validate_schedule(def.schedule, def.persistent) do
      warnings =
        warn_unknown_skills(def.skills, available_skills) ++
          warn_capabilities(def.capabilities) ++
          warn_memory_backend(def.memory_backend)

      {:ok, warnings}
    end
  end

  # --- Frontmatter extraction ---

  defp extract_frontmatter(content) do
    content = String.trim(content)

    unless String.starts_with?(content, "---") do
      {:error, "Agent definition must start with YAML frontmatter (---)"}
    end
    |> case do
      {:error, _} = err ->
        err

      nil ->
        after_first = String.slice(content, 3..-1//1)

        case :binary.match(after_first, "---") do
          {pos, 3} ->
            yaml_str = String.trim(binary_part(after_first, 0, pos))
            body = String.trim(binary_part(after_first, pos + 3, byte_size(after_first) - pos - 3))
            {:ok, yaml_str, body}

          :nomatch ->
            {:error, "Missing closing --- for YAML frontmatter"}
        end
    end
  end

  defp parse_yaml(yaml_str) do
    case YamlElixir.read_from_string(yaml_str) do
      {:ok, map} when is_map(map) -> {:ok, map}
      {:ok, _} -> {:error, "YAML frontmatter must be a mapping"}
      {:error, %YamlElixir.ParsingError{message: msg}} -> {:error, "Invalid YAML: #{msg}"}
    end
  end

  # --- Schema validation via NimbleOptions ---

  defp validate_schema(yaml_map) do
    opts =
      yaml_map
      |> Enum.map(fn {k, v} -> {String.to_existing_atom(k), v} end)

    case NimbleOptions.validate(opts, @nimble_schema) do
      {:ok, validated} -> {:ok, validated}
      {:error, %NimbleOptions.ValidationError{message: msg}} -> {:error, msg}
    end
  rescue
    ArgumentError -> {:error, "Unknown key in YAML frontmatter"}
  end

  defp build_definition(opts, body) do
    memory = parse_memory_isolation(opts[:memory])

    case memory do
      {:ok, mem} ->
        {:ok,
         %__MODULE__{
           name: opts[:name],
           persistent: opts[:persistent],
           skills: opts[:skills],
           memory: mem,
           memory_backend: opts[:memory_backend],
           schedule: opts[:schedule],
           channels: opts[:channels],
           delegates_to: opts[:delegates_to],
           model: opts[:model],
           temperature: normalize_temperature(opts[:temperature]),
           max_tools_per_turn: opts[:max_tools_per_turn],
           allowed_tools: opts[:allowed_tools],
           capabilities: opts[:capabilities],
           personality: body
         }}

      {:error, _} = err ->
        err
    end
  end

  defp normalize_temperature(nil), do: nil
  defp normalize_temperature(t) when is_integer(t), do: t * 1.0
  defp normalize_temperature(t) when is_float(t), do: t

  defp parse_memory_isolation("isolated"), do: {:ok, :isolated}
  defp parse_memory_isolation("shared-read"), do: {:ok, :shared_read}
  defp parse_memory_isolation("shared"), do: {:ok, :shared}

  defp parse_memory_isolation(other),
    do: {:error, "Invalid memory isolation: '#{other}', expected one of: #{Enum.join(@valid_memory_isolations, ", ")}"}

  # --- Validation helpers ---

  defp validate_name(name) do
    cond do
      name == "" or is_nil(name) -> {:error, "Agent name cannot be empty"}
      String.contains?(name, ["/", "\\"]) -> {:error, "Agent name cannot contain path separators"}
      true -> :ok
    end
  end

  defp validate_schedule(nil, _persistent), do: :ok
  defp validate_schedule(_schedule, false), do: {:error, "Schedule requires persistent: true"}
  defp validate_schedule(_schedule, true), do: :ok

  defp warn_unknown_skills(skills, available) do
    for skill <- skills, skill not in available do
      "Skill '#{skill}' not found in workspace"
    end
  end

  defp warn_capabilities(capabilities) do
    for cap <- capabilities, warning = capability_warning(cap), warning != nil do
      warning
    end
  end

  defp capability_warning(""), do: "Capability string cannot be empty"

  defp capability_warning(cap) do
    if snake_case?(cap), do: nil, else: "Capability '#{cap}' is not snake_case (use lowercase letters, digits, and underscores)"
  end

  defp snake_case?(s) do
    Regex.match?(~r/^[a-z_][a-z0-9_]*$/, s)
  end

  defp warn_memory_backend(backend) when backend in @valid_memory_backends, do: []
  defp warn_memory_backend(backend), do: ["Unknown memory_backend '#{backend}', will use markdown"]
end
