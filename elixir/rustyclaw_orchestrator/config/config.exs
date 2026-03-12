import Config

config :logger, :default_formatter,
  metadata: [
    :agent,
    :event,
    :trace_id,
    :origin_agent,
    :source_agent,
    :kind,
    :delegation_depth,
    :from,
    :to,
    :btw_pid,
    :message_preview,
    :reason,
    :elapsed_ms,
    :status,
    :channel,
    :quote_reply
  ]

# Import environment-specific config (must be at the end)
import_config "#{config_env()}.exs"
