defmodule RustyclawOrchestrator.BtwRouter do
  @moduledoc """
  Routes inbound messages to either the main agent queue or the BTW side-channel.

  Messages prefixed with `/btw ` (case-insensitive) are stripped of the prefix
  and dispatched to a fire-and-forget BtwServer. All other messages pass through
  to the main agent unchanged.

  Routing overhead is sub-millisecond: a single binary pattern match on the first
  5 bytes covers the fast path.
  """

  alias RustyclawOrchestrator.{AgentServer, BtwSupervisor}

  require Logger

  @btw_prefix_len 5

  @type route_result ::
          {:btw, pid()}
          | {:main, :ok}
          | {:error, term()}

  @type channel_info :: %{
          optional(:channel) => String.t(),
          optional(:reply_to_message_id) => String.t() | integer(),
          optional(:chat_id) => String.t() | integer()
        }

  @doc """
  Route a message to the appropriate handler.

  If the message starts with `/btw ` (case-insensitive), it is stripped and
  dispatched as a parallel side-channel task. Otherwise it is forwarded to the
  main agent via `AgentServer.send_message/2`.

  ## Parameters

    - `agent_name` — the target agent's registered name
    - `message` — the raw inbound message text
    - `opts` — keyword options:
      - `:channel_info` — map with channel/reply routing metadata
      - `:provenance` — optional `MessageProvenance` for tracing

  ## Returns

    - `{:btw, pid}` — message routed to a BTW side-channel process
    - `{:main, :ok}` — message forwarded to the main agent
    - `{:error, reason}` — routing failed
  """
  @spec route(String.t(), String.t(), keyword()) :: route_result()
  def route(agent_name, message, opts \\ []) when is_binary(agent_name) and is_binary(message) do
    if btw_message?(message) do
      stripped = strip_prefix(message)
      channel_info = Keyword.get(opts, :channel_info, %{})
      provenance = Keyword.get(opts, :provenance)
      dispatch_btw(agent_name, stripped, channel_info, provenance)
    else
      provenance = Keyword.get(opts, :provenance)
      AgentServer.send_message(agent_name, message, provenance)
      {:main, :ok}
    end
  end

  @doc """
  Check whether a message is a BTW side-channel message.

  Matches `/btw ` prefix case-insensitively. The space after `/btw` is required
  to avoid false positives on `/btweet` etc.
  """
  @spec btw_message?(String.t()) :: boolean()
  def btw_message?(<<prefix::binary-size(@btw_prefix_len), _rest::binary>>) do
    String.downcase(prefix) == "/btw "
  end

  def btw_message?(_), do: false

  @doc """
  Strip the `/btw ` prefix from a message. Returns the original if no prefix.
  """
  @spec strip_prefix(String.t()) :: String.t()
  def strip_prefix(<<prefix::binary-size(@btw_prefix_len), rest::binary>>) do
    if String.downcase(prefix) == "/btw ", do: rest, else: prefix <> rest
  end

  def strip_prefix(message), do: message

  # --- Internals ---

  defp dispatch_btw(agent_name, message, channel_info, provenance) do
    context = fetch_agent_context(agent_name)

    case BtwSupervisor.start_btw(
           message: message,
           agent_name: agent_name,
           context: context,
           channel_info: channel_info,
           provenance: provenance
         ) do
      {:ok, pid} ->
        Logger.info("BTW side-channel started",
          agent: agent_name,
          btw_pid: inspect(pid),
          message_preview: String.slice(message, 0, 50)
        )

        {:btw, pid}

      {:error, reason} ->
        Logger.warning("BTW dispatch failed",
          agent: agent_name,
          reason: inspect(reason)
        )

        {:error, reason}
    end
  end

  defp fetch_agent_context(agent_name) do
    case Registry.lookup(RustyclawOrchestrator.AgentRegistry, agent_name) do
      [{_pid, _}] ->
        try do
          state = AgentServer.get_state(agent_name)

          %{
            accumulated_state: state.accumulated_state,
            definition: state.definition,
            session_id: state.session_id
          }
        catch
          :exit, _ ->
            %{accumulated_state: %{}, definition: nil, session_id: nil}
        end

      [] ->
        %{accumulated_state: %{}, definition: nil, session_id: nil}
    end
  end
end
