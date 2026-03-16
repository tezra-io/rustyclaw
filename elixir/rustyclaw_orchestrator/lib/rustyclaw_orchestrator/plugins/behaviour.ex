defmodule RustyclawOrchestrator.Plugins.Behaviour do
  @moduledoc """
  Behaviour contract for agent plugins.

  Each plugin implements these 6 callbacks to integrate with the plugin system.
  Plugins connect to external services, execute tasks with streaming callbacks,
  report health, and manage rate limits.
  """

  @doc "Establish connection to external agent/service."
  @callback connect(config :: map()) :: {:ok, state :: term()} | {:error, reason :: term()}

  @doc """
  Execute a task with streaming event callbacks.

  Returns `:tool_use` when tool calls are needed (Worker handles execution and loops back),
  or `:complete` when the task is finished.

  The `event_handler` is called during execution for streaming progress:
  - `{:chunk, text}` — raw streaming output
  - `{:tool_use, name, args}` — tool call detected (informational)
  - `{:tool_result, result}` — tool execution completed
  - `{:artifact, type, content}` — code block, file edit, etc.
  """
  @callback execute(state :: term(), task :: map(), event_handler :: (term() -> :ok)) ::
              {:ok, {:tool_use, tool_calls :: list()}, new_state :: term()}
              | {:ok, {:complete, result :: term()}, new_state :: term()}
              | {:error, reason :: term()}

  @doc "Check plugin liveness."
  @callback health(state :: term()) :: :healthy | :degraded | :unhealthy

  @doc "List capabilities this plugin provides."
  @callback capabilities() :: [atom()]

  @doc "Current rate limit status."
  @callback rate_limit_status(state :: term()) ::
              %{remaining: non_neg_integer(), reset_at: DateTime.t() | nil, limited: boolean()}

  @doc "Clean shutdown."
  @callback disconnect(state :: term()) :: :ok
end
