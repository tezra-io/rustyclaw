defmodule RustyclawOrchestrator.Plugins.AutoRouter do
  @moduledoc """
  Routes tasks to capabilities based on Linear issue labels.

  Examines `plugin:*` labels to determine which capability set
  a task requires, enabling automatic dispatch to the correct
  Worker plugin.
  """

  @label_capability_map %{
    "plugin:coding" => [:coding],
    "plugin:review" => [:review],
    "plugin:analysis" => [:analysis]
  }

  @default_capabilities [:coding]

  @doc """
  Determine capabilities for a task based on its labels.

  Looks for labels matching `plugin:<capability>` patterns.
  Returns `[:coding]` when no matching labels are found.

  ## Examples

      iex> AutoRouter.route_task(%{labels: ["plugin:review", "bug"]})
      [:review]

      iex> AutoRouter.route_task(%{labels: ["plugin:coding", "plugin:review"]})
      [:coding, :review]

      iex> AutoRouter.route_task(%{labels: ["bug", "urgent"]})
      [:coding]
  """
  @spec route_task(map()) :: [atom()]
  def route_task(task) do
    labels = task[:labels] || []

    capabilities =
      labels
      |> Enum.flat_map(fn label -> Map.get(@label_capability_map, label, []) end)
      |> Enum.uniq()

    case capabilities do
      [] -> @default_capabilities
      caps -> caps
    end
  end
end
