defmodule RustyclawOrchestrator.RustBridge do
  @moduledoc """
  HTTP bridge to the Rust/RustyClaw core.

  GenServer wrapping a Req HTTP client that communicates with the Rust binary
  over localhost. HTTP calls and retries run in supervised Task workers so the
  GenServer stays responsive for concurrent callers.

  The Rust core exposes endpoints like:
  - POST /api/agent/run — execute an agent task
  - GET  /api/health    — health check
  """

  use GenServer

  alias RustyclawOrchestrator.MessageProvenance

  @default_base_url "http://localhost:4200"
  @call_timeout 60_000
  @max_retries 3
  @initial_backoff_ms 500
  @task_supervisor RustyclawOrchestrator.RustBridge.TaskSupervisor

  # --- Client API ---

  def start_link(opts \\ []) do
    {name, init_opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, init_opts, name: name)
  end

  @doc """
  Execute an agent task via the Rust core.

  Opts supports `:provenance` (`MessageProvenance.t()`) to include trace
  metadata in the JSON payload sent to Rust.
  """
  @spec run_task(String.t(), String.t(), keyword()) :: {:ok, map()} | {:error, term()}
  def run_task(agent_name, task, opts \\ []) do
    GenServer.call(
      __MODULE__,
      {:run_task, agent_name, task, opts},
      @call_timeout
    )
  end

  @doc "Check if the Rust core is reachable."
  @spec health_check() :: :ok | {:error, term()}
  def health_check do
    GenServer.call(__MODULE__, :health_check, @call_timeout)
  end

  @doc "Get the configured base URL."
  @spec base_url() :: String.t()
  def base_url do
    GenServer.call(__MODULE__, :base_url, 5_000)
  end

  # --- GenServer Callbacks ---

  @impl true
  def init(opts) do
    base_url = Keyword.get(opts, :base_url, @default_base_url)
    max_retries = Keyword.get(opts, :max_retries, @max_retries)
    connect_timeout = Keyword.get(opts, :connect_timeout, 5_000)

    state = %{
      base_url: base_url,
      max_retries: max_retries,
      req: build_req(base_url, connect_timeout),
      pending: %{}
    }

    {:ok, state}
  end

  @impl true
  def handle_call({:run_task, agent_name, task, opts}, from, state) do
    provenance = Keyword.get(opts, :provenance)

    body =
      %{
        agent: agent_name,
        task: task,
        model: Keyword.get(opts, :model),
        temperature: Keyword.get(opts, :temperature)
      }
      |> maybe_add_provenance(provenance)

    req = state.req
    max_retries = state.max_retries

    %Task{ref: ref} =
      Task.Supervisor.async_nolink(@task_supervisor, fn ->
        post_with_retry(req, "/api/agent/run", body, 0, max_retries)
      end)

    {:noreply, %{state | pending: Map.put(state.pending, ref, from)}}
  end

  def handle_call(:health_check, from, state) do
    req = state.req

    %Task{ref: ref} =
      Task.Supervisor.async_nolink(@task_supervisor, fn ->
        case Req.get(req, url: "/api/health") do
          {:ok, %Req.Response{status: status}} when status in 200..299 -> :ok
          {:ok, %Req.Response{status: status}} -> {:error, {:http_error, status}}
          {:error, reason} -> {:error, reason}
        end
      end)

    {:noreply, %{state | pending: Map.put(state.pending, ref, from)}}
  end

  def handle_call(:base_url, _from, state) do
    {:reply, state.base_url, state}
  end

  @impl true
  def handle_info({ref, result}, state) when is_reference(ref) do
    Process.demonitor(ref, [:flush])
    {from, pending} = Map.pop(state.pending, ref)
    if from, do: GenServer.reply(from, result)
    {:noreply, %{state | pending: pending}}
  end

  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    {from, pending} = Map.pop(state.pending, ref)
    if from, do: GenServer.reply(from, {:error, {:task_crashed, reason}})
    {:noreply, %{state | pending: pending}}
  end

  # --- Internals ---

  defp build_req(base_url, connect_timeout) do
    Req.new(
      base_url: base_url,
      headers: [{"content-type", "application/json"}],
      receive_timeout: 30_000,
      connect_options: [timeout: connect_timeout],
      # Disable Req's built-in retry — we handle retries ourselves
      retry: false
    )
  end

  defp post_with_retry(_req, _path, _body, attempt, max_retries) when attempt >= max_retries do
    {:error, :max_retries_exceeded}
  end

  defp post_with_retry(req, path, body, attempt, max_retries) do
    case Req.post(req, url: path, json: body) do
      {:ok, %Req.Response{status: status, body: resp_body}} when status in 200..299 ->
        {:ok, resp_body}

      {:ok, %Req.Response{status: status, body: _resp_body}} when status >= 500 ->
        # Server error — retry with backoff
        backoff = @initial_backoff_ms * Integer.pow(2, attempt)
        Process.sleep(backoff)
        post_with_retry(req, path, body, attempt + 1, max_retries)

      {:ok, %Req.Response{status: status, body: resp_body}} ->
        # Client error — don't retry
        {:error, {:http_error, status, resp_body}}

      {:error, %Req.TransportError{reason: reason}} when reason in [:econnrefused, :timeout] ->
        # Connection error — retry with backoff
        backoff = @initial_backoff_ms * Integer.pow(2, attempt)
        Process.sleep(backoff)
        post_with_retry(req, path, body, attempt + 1, max_retries)

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp maybe_add_provenance(body, %MessageProvenance{} = prov) do
    Map.put(body, :provenance, MessageProvenance.to_map(prov))
  end

  defp maybe_add_provenance(body, _), do: body
end
