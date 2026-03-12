defmodule RustyclawOrchestrator.BtwSupervisor do
  @moduledoc """
  DynamicSupervisor for BTW side-channel tasks.

  Each `/btw` message spawns a temporary `BtwServer` under this supervisor.
  Processes are `:temporary` — they are never restarted after termination.

  The supervisor allows a high restart intensity since BTW tasks are independent
  fire-and-forget operations that should not affect each other.
  """

  @doc """
  Start a new BTW side-channel task under supervision.

  ## Required opts

    - `:message` — the stripped BTW message text
    - `:agent_name` — the parent agent's name
    - `:context` — snapshot of the main agent's context
    - `:channel_info` — channel routing metadata

  ## Optional opts

    - `:provenance` — trace metadata
  """
  @spec start_btw(keyword()) :: {:ok, pid()} | {:error, term()}
  def start_btw(opts) do
    child_spec = %{
      id: RustyclawOrchestrator.BtwServer,
      start: {RustyclawOrchestrator.BtwServer, :start_link, [opts]},
      restart: :temporary
    }

    DynamicSupervisor.start_child(__MODULE__, child_spec)
  end

  @doc "Count active BTW tasks."
  @spec count_active() :: non_neg_integer()
  def count_active do
    DynamicSupervisor.count_children(__MODULE__).active
  end

  @doc "List pids of active BTW tasks."
  @spec list_active() :: [pid()]
  def list_active do
    __MODULE__
    |> DynamicSupervisor.which_children()
    |> Enum.map(fn {_, pid, _, _} -> pid end)
    |> Enum.filter(&is_pid/1)
  end
end
