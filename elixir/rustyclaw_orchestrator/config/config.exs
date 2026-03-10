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
    :to
  ]
