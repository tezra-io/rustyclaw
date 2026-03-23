defmodule RustyclawOrchestrator.BtwServer do
  @moduledoc """
  Fire-and-forget GenServer for BTW side-channel tasks.

  Each BtwServer handles exactly one `/btw` message: it calls the Rust core
  via RustBridge for LLM inference + tool execution, sends the response back
  on the originating channel (with quote-reply where supported), then terminates.

  ## Lifecycle

      start_link(opts) → init → execute_task → send_response → {:stop, :normal}

  The process is supervised under `BtwSupervisor` (DynamicSupervisor) with
  `:temporary` restart strategy — it is never restarted after completion.

  ## Resource Contention

  Before executing, the server checks `ResourceLock` for exclusive resources
  (e.g., browser). If the resource is busy, it waits briefly or returns a
  graceful error. Main agent tasks always have priority.
  """

  use GenServer, restart: :temporary

  alias RustyclawOrchestrator.{ResourceLock, RustBridge}

  require Logger

  @task_timeout 120_000
  @exclusive_resources ["browser"]

  # --- Client API ---

  @doc """
  Start a BTW server for a single side-channel task.

  ## Required opts

    - `:message` — the stripped BTW message text
    - `:agent_name` — the parent agent's name (for RustBridge routing)
    - `:context` — snapshot of the main agent's accumulated state
    - `:channel_info` — channel routing metadata for reply delivery

  ## Optional opts

    - `:provenance` — `MessageProvenance` for trace propagation
  """
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    state = %{
      message: Keyword.fetch!(opts, :message),
      agent_name: Keyword.fetch!(opts, :agent_name),
      context: Keyword.fetch!(opts, :context),
      channel_info: Keyword.get(opts, :channel_info, %{}),
      provenance: Keyword.get(opts, :provenance),
      started_at: System.monotonic_time(:millisecond)
    }

    # Kick off execution asynchronously so init returns immediately
    send(self(), :execute)
    {:ok, state}
  end

  @impl true
  def handle_info(:execute, state) do
    result = execute_with_resource_check(state)
    send_response(result, state)

    elapsed = System.monotonic_time(:millisecond) - state.started_at

    Logger.info("BTW task completed",
      agent: state.agent_name,
      elapsed_ms: elapsed,
      status: elem(result, 0)
    )

    {:stop, :normal, state}
  end

  def handle_info(_msg, state) do
    {:noreply, state}
  end

  # --- Internals ---

  defp execute_with_resource_check(state) do
    case check_exclusive_resources(state.message) do
      {:ok, acquired} ->
        execute_task(state, acquired)

      {:error, :resource_busy} = err ->
        err
    end
  end

  defp check_exclusive_resources(message) do
    needed = detect_exclusive_resources(message)

    Enum.reduce_while(needed, {:ok, []}, fn resource, {:ok, acquired} ->
      case ResourceLock.acquire(resource, wait_ms: 2_000, priority: :btw) do
        :ok ->
          {:cont, {:ok, [resource | acquired]}}

        {:error, _} = err ->
          release_resources(acquired)
          {:halt, err}
      end
    end)
  end

  defp detect_exclusive_resources(message) do
    msg_lower = String.downcase(message)

    Enum.filter(@exclusive_resources, fn resource ->
      String.contains?(msg_lower, resource)
    end)
  end

  defp execute_task(state, acquired_resources) do
    task =
      Task.async(fn ->
        RustBridge.run_task(state.agent_name, state.message,
          provenance: state.provenance,
          context: state.context
        )
      end)

    case Task.yield(task, @task_timeout) || Task.shutdown(task) do
      {:ok, result} -> result
      {:exit, reason} -> {:error, {:task_exit, reason}}
      nil -> {:error, :timeout}
    end
  after
    release_resources(acquired_resources)
  end

  defp release_resources(resources) do
    Enum.each(resources, fn resource ->
      ResourceLock.release(resource)
    end)
  end

  defp send_response(result, state) do
    response_text = format_response(result)
    channel_info = state.channel_info

    reply_payload = %{
      text: response_text,
      channel: Map.get(channel_info, "channel"),
      reply_to_message_id: Map.get(channel_info, "reply_to_message_id"),
      chat_id: Map.get(channel_info, "chat_id"),
      btw: true
    }

    Logger.debug("BTW response ready",
      agent: state.agent_name,
      channel: reply_payload.channel,
      quote_reply: reply_payload.reply_to_message_id != nil
    )

    case RustBridge.send_to_channel(reply_payload) do
      {:ok, _resp} ->
        Logger.info("BTW response delivered",
          agent: state.agent_name,
          channel: reply_payload.channel
        )

        {:ok, reply_payload}

      {:error, reason} ->
        Logger.error("BTW response delivery failed",
          agent: state.agent_name,
          reason: inspect(reason)
        )

        {:error, {:delivery_failed, reason}}
    end
  end

  defp format_response({:ok, %{} = body}) do
    Map.get(body, "response", Map.get(body, :response, inspect(body)))
  end

  defp format_response({:error, :resource_busy}) do
    "Sorry, I can't handle that right now — an exclusive resource (e.g., browser) " <>
      "is in use by the main task. Try again in a moment."
  end

  defp format_response({:error, :timeout}) do
    "The side-channel task timed out. The main agent may be under heavy load."
  end

  defp format_response({:error, reason}) do
    Logger.error("BTW task failed", reason: inspect(reason))
    "The side-channel task encountered an error. Please try again."
  end
end
