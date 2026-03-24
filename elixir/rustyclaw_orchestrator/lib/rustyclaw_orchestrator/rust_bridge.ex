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

  require Logger

  alias RustyclawOrchestrator.MessageProvenance

  @default_base_url "http://localhost:42617"
  @startup_health_timeout 5_000
  @call_timeout 60_000
  @run_task_timeout 360_000
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
      @run_task_timeout
    )
  end

  @doc """
  Send a message to a channel via the Rust core.

  Used by BtwServer to deliver responses back to the originating channel.
  """
  @spec send_to_channel(map()) :: {:ok, map()} | {:error, term()}
  def send_to_channel(payload) when is_map(payload) do
    GenServer.call(__MODULE__, {:send_to_channel, payload}, @call_timeout)
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
    {base_url, unix_socket} = resolve_connection(opts)
    max_retries = Keyword.get(opts, :max_retries, @max_retries)
    connect_timeout = Keyword.get(opts, :connect_timeout, 5_000)

    transport =
      if unix_socket,
        do: "UDS (#{unix_socket})",
        else: "TCP (#{base_url})"

    Logger.info("RustBridge starting — #{transport}")

    state = %{
      base_url: base_url,
      unix_socket: unix_socket,
      max_retries: max_retries,
      req: build_req(base_url, connect_timeout, unix_socket),
      pending: %{}
    }

    # Fire-and-forget startup health check so we don't block the supervisor.
    # Skip in test — Bypass mocks don't expect unsolicited requests.
    unless Keyword.get(opts, :skip_health_check, false) do
      send(self(), :startup_health_check)
    end

    {:ok, state}
  end

  # Resolve connection params: explicit opts → Application config → defaults.
  # Returns {base_url, unix_socket | nil}.
  defp resolve_connection(opts) do
    explicit_socket = Keyword.get(opts, :unix_socket)
    explicit_url = Keyword.get(opts, :base_url)

    cond do
      explicit_socket ->
        {explicit_url || "http://localhost", explicit_socket}

      explicit_url ->
        {explicit_url, nil}

      config = Application.get_env(:rustyclaw_orchestrator, :rust_bridge) ->
        url = Keyword.get(config, :base_url, @default_base_url)
        socket = Keyword.get(config, :unix_socket)
        {url, socket}

      true ->
        {@default_base_url, nil}
    end
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

  @impl true
  def handle_call({:send_to_channel, payload}, from, state) do
    req = state.req
    max_retries = state.max_retries

    %Task{ref: ref} =
      Task.Supervisor.async_nolink(@task_supervisor, fn ->
        post_with_retry(req, "/api/channel/send", payload, 0, max_retries)
      end)

    {:noreply, %{state | pending: Map.put(state.pending, ref, from)}}
  end

  @impl true
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

  @impl true
  def handle_call(:base_url, _from, state) do
    {:reply, state.base_url, state}
  end

  @impl true
  def handle_info(:startup_health_check, state) do
    target = connection_label(state)

    case Req.get(state.req, url: "/api/health", receive_timeout: @startup_health_timeout) do
      {:ok, %Req.Response{status: status}} when status in 200..299 ->
        Logger.info("RustBridge connected to Rust core via #{target}")

      {:ok, %Req.Response{status: status}} ->
        Logger.error(
          "RustBridge health check failed — Rust core at #{target} returned HTTP #{status}. " <>
            "Check that the gateway is running."
        )

      {:error, reason} ->
        Logger.error(
          "RustBridge cannot reach Rust core at #{target}: #{inspect(reason)}. " <>
            "Ensure the gateway is running."
        )
    end

    {:noreply, state}
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

  defp build_req(base_url, connect_timeout, unix_socket) do
    base_opts = [
      base_url: base_url,
      headers: [{"content-type", "application/json"}],
      receive_timeout: 30_000,
      connect_options: [timeout: connect_timeout],
      # Disable Req's built-in retry — we handle retries ourselves
      retry: false
    ]

    opts =
      if unix_socket do
        Keyword.put(base_opts, :unix_socket, unix_socket)
      else
        base_opts
      end

    Req.new(opts)
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

  defp connection_label(%{unix_socket: socket}) when is_binary(socket), do: "UDS (#{socket})"
  defp connection_label(%{base_url: url}), do: url
end
