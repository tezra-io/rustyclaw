defmodule RustyclawOrchestrator.ResourceLock do
  @moduledoc """
  ETS-based resource locking for exclusive resources (e.g., browser).

  Main agent tasks have priority over BTW side-channel tasks. When a `:main`
  priority task acquires a resource held by a `:btw` task, the BTW lock is
  preempted: the ETS entry is replaced and the preempted process receives a
  `{:resource_preempted, resource}` message for graceful cleanup.

  BTW tasks cannot preempt main tasks — they wait or time out normally.
  Same-priority tasks also wait normally (no preemption).

  Lock granularity is per resource name (e.g., `"browser"`, `"serial_port"`).
  """

  @table __MODULE__
  @default_wait_ms 2_000
  @poll_interval_ms 50

  @type priority :: :main | :btw

  @doc """
  Initialize the ETS table. Called once at application startup.
  """
  @spec init() :: :ok
  def init do
    :ets.new(@table, [:set, :public, :named_table, read_concurrency: true])
    :ok
  end

  @doc """
  Acquire an exclusive lock on a resource.

  Returns `:ok` if the lock was acquired, `{:error, :resource_busy}` if the
  resource is held by another process after waiting `wait_ms` milliseconds.

  The lock is automatically released if the holding process dies (monitored).

  ## Options

    - `:wait_ms` — max time to wait for the lock (default: #{@default_wait_ms}ms)
    - `:priority` — `:main` or `:btw` (default: `:btw`). Main tasks preempt BTW locks.
  """
  @spec acquire(String.t(), keyword()) :: :ok | {:error, :resource_busy}
  def acquire(resource, opts \\ []) when is_binary(resource) do
    wait_ms = Keyword.get(opts, :wait_ms, @default_wait_ms)
    priority = Keyword.get(opts, :priority, :btw)
    deadline = System.monotonic_time(:millisecond) + wait_ms
    try_acquire(resource, priority, deadline)
  end

  @doc """
  Release a previously acquired lock. Only the holding process can release.
  """
  @spec release(String.t()) :: :ok | {:error, :not_held}
  def release(resource) when is_binary(resource) do
    case :ets.lookup(@table, resource) do
      [{^resource, pid, _ref, _priority}] when pid == self() ->
        :ets.delete(@table, resource)
        :ok

      _ ->
        {:error, :not_held}
    end
  end

  @doc """
  Check if a resource is currently locked.
  """
  @spec locked?(String.t()) :: boolean()
  def locked?(resource) when is_binary(resource) do
    case :ets.lookup(@table, resource) do
      [{^resource, pid, _ref, _priority}] -> Process.alive?(pid)
      [] -> false
    end
  end

  @doc """
  Get the holder pid for a resource, or nil if unlocked.
  """
  @spec holder(String.t()) :: pid() | nil
  def holder(resource) when is_binary(resource) do
    case :ets.lookup(@table, resource) do
      [{^resource, pid, _ref, _priority}] ->
        if Process.alive?(pid), do: pid, else: nil

      [] ->
        nil
    end
  end

  # --- Internals ---

  defp try_acquire(resource, priority, deadline) do
    lock_ref = make_ref()

    case :ets.insert_new(@table, {resource, self(), lock_ref, priority}) do
      true ->
        monitor_holder(resource, self(), lock_ref)
        :ok

      false ->
        try_acquire_contested(resource, priority, lock_ref, deadline)
    end
  end

  defp try_acquire_contested(resource, priority, lock_ref, deadline) do
    if maybe_preempt(resource, priority, lock_ref) do
      :ok
    else
      maybe_reclaim_dead_lock(resource)
      retry_or_timeout(resource, priority, deadline)
    end
  end

  defp retry_or_timeout(resource, priority, deadline) do
    if System.monotonic_time(:millisecond) < deadline do
      Process.sleep(@poll_interval_ms)
      try_acquire(resource, priority, deadline)
    else
      {:error, :resource_busy}
    end
  end

  # A :main task can preempt a :btw holder. Returns true if preemption occurred.
  defp maybe_preempt(resource, :main, lock_ref) do
    case :ets.lookup(@table, resource) do
      [{^resource, holder_pid, _old_ref, :btw}] when holder_pid != self() ->
        :ets.insert(@table, {resource, self(), lock_ref, :main})
        monitor_holder(resource, self(), lock_ref)
        send(holder_pid, {:resource_preempted, resource})
        true

      _ ->
        false
    end
  end

  defp maybe_preempt(_resource, :btw, _lock_ref), do: false

  defp maybe_reclaim_dead_lock(resource) do
    case :ets.lookup(@table, resource) do
      [{^resource, pid, _ref, _priority}] ->
        unless Process.alive?(pid) do
          :ets.delete(@table, resource)
        end

      [] ->
        :ok
    end
  end

  defp monitor_holder(resource, holder_pid, lock_ref) do
    spawn(fn ->
      mon_ref = Process.monitor(holder_pid)

      receive do
        {:DOWN, ^mon_ref, :process, ^holder_pid, _reason} ->
          case :ets.lookup(@table, resource) do
            [{^resource, ^holder_pid, ^lock_ref, _priority}] ->
              :ets.delete(@table, resource)

            _ ->
              :ok
          end
      end
    end)
  end
end
