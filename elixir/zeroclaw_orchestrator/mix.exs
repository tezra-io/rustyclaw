defmodule ZeroclawOrchestrator.MixProject do
  use Mix.Project

  def project do
    [
      app: :zeroclaw_orchestrator,
      version: "0.1.0",
      elixir: "~> 1.17",
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      aliases: aliases()
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {ZeroclawOrchestrator.Application, []}
    ]
  end

  defp deps do
    [
      {:jason, "~> 1.4"},
      {:yaml_elixir, "~> 2.11"},
      {:nimble_options, "~> 1.1"},
      {:req, "~> 0.5"},

      # Dev/test
      {:credo, "~> 1.7", only: [:dev, :test], runtime: false}
    ]
  end

  defp aliases do
    [
      lint: ["format --check-formatted", "credo --strict"]
    ]
  end
end
