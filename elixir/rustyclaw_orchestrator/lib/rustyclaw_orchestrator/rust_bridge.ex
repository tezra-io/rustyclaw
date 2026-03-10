defmodule RustyclawOrchestrator.RustBridge do
  @moduledoc """
  HTTP bridge to the Rust/RustyClaw core.

  GenServer wrapping a Req HTTP client that communicates with the Rust binary
  over localhost. Provides retry with exponential backoff.

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

  # --- Client API ---

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
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

    state = %{
      base_url: base_url,
      req: build_req(base_url)
    }

    {:ok, state}
  end

  @impl true
  def handle_call({:run_task, agent_name, task, opts}, _from, state) do
    provenance = Keyword.get(opts, :provenance)

    body =
      %{
        agent: agent_name,
        task: task,
        model: Keyword.get(opts, :model),
        temperature: Keyword.get(opts, :temperature)
      }
      |> maybe_add_provenance(provenance)

    result = post_with_retry(state.req, "/api/agent/run", body)
    {:reply, result, state}
  end

  def handle_call(:health_check, _from, state) do
    result =
      case Req.get(state.req, url: "/api/health") do
        {:ok, %Req.Response{status: status}} when status in 200..299 -> :ok
        {:ok, %Req.Response{status: status}} -> {:error, {:http_error, status}}
        {:error, reason} -> {:error, reason}
      end

    {:reply, result, state}
  end

  def handle_call(:base_url, _from, state) do
    {:reply, state.base_url, state}
  end

  # --- Internals ---

  defp build_req(base_url) do
    Req.new(
      base_url: base_url,
      headers: [{"content-type", "application/json"}],
      receive_timeout: 30_000,
      connect_options: [timeout: 5_000],
      # Disable Req's built-in retry — we handle retries ourselves
      retry: false
    )
  end

  defp post_with_retry(req, path, body) do
    post_with_retry(req, path, body, 0)
  end

  defp post_with_retry(_req, _path, _body, attempt) when attempt >= @max_retries do
    {:error, :max_retries_exceeded}
  end

  defp post_with_retry(req, path, body, attempt) do
    case Req.post(req, url: path, json: body) do
      {:ok, %Req.Response{status: status, body: resp_body}} when status in 200..299 ->
        {:ok, resp_body}

      {:ok, %Req.Response{status: status, body: _resp_body}} when status >= 500 ->
        # Server error — retry with backoff
        backoff = @initial_backoff_ms * Integer.pow(2, attempt)
        Process.sleep(backoff)
        post_with_retry(req, path, body, attempt + 1)

      {:ok, %Req.Response{status: status, body: resp_body}} ->
        # Client error — don't retry
        {:error, {:http_error, status, resp_body}}

      {:error, %Req.TransportError{reason: reason}} when reason in [:econnrefused, :timeout] ->
        # Connection error — retry with backoff
        backoff = @initial_backoff_ms * Integer.pow(2, attempt)
        Process.sleep(backoff)
        post_with_retry(req, path, body, attempt + 1)

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp maybe_add_provenance(body, %MessageProvenance{} = prov) do
    Map.put(body, :provenance, MessageProvenance.to_map(prov))
  end

  defp maybe_add_provenance(body, _), do: body
end
