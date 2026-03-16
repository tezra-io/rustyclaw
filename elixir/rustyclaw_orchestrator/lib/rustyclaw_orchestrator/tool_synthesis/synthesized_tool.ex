defmodule RustyclawOrchestrator.ToolSynthesis.SynthesizedTool do
  @moduledoc """
  Behaviour that all synthesized tools must implement.

  Maps 1:1 to Rust's `Tool` trait — name, description, schema, execute.
  """

  @callback name() :: String.t()
  @callback description() :: String.t()
  @callback parameters_schema() :: map()
  @callback execute(params :: map()) :: {:ok, String.t()} | {:error, String.t()}

  @callback capabilities() :: [String.t()]
  @optional_callbacks [capabilities: 0]
end
