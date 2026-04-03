defmodule RustyclawOrchestrator.SkillRegistry do
  @moduledoc """
  Loads skill templates from the workspace skills directory.

  Skills are stored as `~/.rustyclaw/workspace/skills/<name>/SKILL.md` using the
  same YAML frontmatter + markdown format as agent definitions.
  """

  alias RustyclawOrchestrator.AgentDefinition

  @default_skills_dir "~/.rustyclaw/workspace/skills"

  @doc "Load a skill definition by name."
  @spec load(String.t()) :: {:ok, AgentDefinition.t()} | {:error, String.t()}
  def load(skill_name) when is_binary(skill_name) do
    if skill_name =~ ~r/\A[a-zA-Z0-9_\-]+\z/ do
      path = Path.join([skills_dir(), skill_name, "SKILL.md"])
      AgentDefinition.from_file(path)
    else
      {:error, "invalid skill name: must match [a-zA-Z0-9_-]+"}
    end
  end

  @doc "List available skill names."
  @spec list() :: [String.t()]
  def list do
    dir = skills_dir()

    case File.ls(dir) do
      {:ok, entries} ->
        entries
        |> Enum.filter(fn entry ->
          full = Path.join(dir, entry)
          File.dir?(full) and File.exists?(Path.join(full, "SKILL.md"))
        end)
        |> Enum.sort()

      {:error, _} ->
        []
    end
  end

  @doc "List skills with parsed definitions."
  @spec list_detailed() :: [AgentDefinition.t()]
  def list_detailed do
    list()
    |> Enum.flat_map(fn name ->
      case load(name) do
        {:ok, def} -> [def]
        {:error, _} -> []
      end
    end)
  end

  defp skills_dir do
    Application.get_env(:rustyclaw_orchestrator, :skills_dir, @default_skills_dir)
    |> Path.expand()
  end
end
