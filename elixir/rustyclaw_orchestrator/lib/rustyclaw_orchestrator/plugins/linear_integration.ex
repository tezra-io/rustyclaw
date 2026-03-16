defmodule RustyclawOrchestrator.Plugins.LinearIntegration do
  @moduledoc """
  Linear API integration for the plugin system.

  Provides GraphQL-based access to Linear for fetching issues,
  updating state, and creating comments.
  """

  require Logger

  @linear_api_url "https://api.linear.app/graphql"

  @doc """
  Fetch unstarted (Todo) issues for a given team.

  Options:
  - `:api_key` — Linear API key (default: from `LINEAR_API_KEY` env var)
  - `:limit` — max issues to return (default: 50)
  - `:http_client` — module with `post/3` for testing (default: uses Req)
  """
  @spec fetch_todo_issues(String.t(), keyword()) :: {:ok, [map()]} | {:error, term()}
  def fetch_todo_issues(team_key, opts \\ []) do
    api_key = Keyword.get_lazy(opts, :api_key, fn -> System.get_env("LINEAR_API_KEY") end)
    limit = Keyword.get(opts, :limit, 50)

    query = """
    query($teamKey: String!, $limit: Int!) {
      issues(
        filter: {
          team: { key: { eq: $teamKey } }
          state: { type: { eq: "unstarted" } }
        }
        first: $limit
        orderBy: priority
      ) {
        nodes {
          id
          identifier
          title
          description
          priority
          labels { nodes { name } }
        }
      }
    }
    """

    variables = %{teamKey: team_key, limit: limit}

    case graphql_request(query, variables, api_key, opts) do
      {:ok, %{"data" => %{"issues" => %{"nodes" => nodes}}}} ->
        issues = Enum.map(nodes, &normalize_issue/1)
        {:ok, issues}

      {:ok, %{"errors" => errors}} ->
        {:error, {:graphql_errors, errors}}

      {:error, reason} ->
        {:error, reason}
    end
  end

  @doc """
  Update an issue's workflow state.

  State must be one of: `:started`, `:completed`, `:failed`, `:cancelled`.

  Options:
  - `:api_key` — Linear API key
  - `:http_client` — module with `post/3` for testing
  """
  @spec update_issue_state(String.t(), atom(), keyword()) :: :ok | {:error, term()}
  def update_issue_state(identifier, state, opts \\ []) do
    api_key = Keyword.get_lazy(opts, :api_key, fn -> System.get_env("LINEAR_API_KEY") end)

    state_name = state_to_linear_name(state)

    query = """
    mutation($identifier: String!, $stateName: String!) {
      issueUpdate(
        identifier: $identifier
        input: { stateId: $stateName }
      ) {
        success
      }
    }
    """

    variables = %{identifier: identifier, stateName: state_name}

    case graphql_request(query, variables, api_key, opts) do
      {:ok, %{"data" => %{"issueUpdate" => %{"success" => true}}}} -> :ok
      {:ok, %{"errors" => errors}} -> {:error, {:graphql_errors, errors}}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc """
  Create a comment on a Linear issue.

  Options:
  - `:api_key` — Linear API key
  - `:http_client` — module with `post/3` for testing
  """
  @spec create_comment(String.t(), String.t(), keyword()) :: :ok | {:error, term()}
  def create_comment(issue_id, body, opts \\ []) do
    api_key = Keyword.get_lazy(opts, :api_key, fn -> System.get_env("LINEAR_API_KEY") end)

    query = """
    mutation($issueId: String!, $body: String!) {
      commentCreate(input: { issueId: $issueId, body: $body }) {
        success
      }
    }
    """

    variables = %{issueId: issue_id, body: body}

    case graphql_request(query, variables, api_key, opts) do
      {:ok, %{"data" => %{"commentCreate" => %{"success" => true}}}} -> :ok
      {:ok, %{"errors" => errors}} -> {:error, {:graphql_errors, errors}}
      {:error, reason} -> {:error, reason}
    end
  end

  # --- Internals ---

  defp graphql_request(_query, _variables, nil, _opts) do
    {:error, :missing_api_key}
  end

  defp graphql_request(query, variables, api_key, opts) do
    body = Jason.encode!(%{query: query, variables: variables})
    headers = [{"authorization", api_key}, {"content-type", "application/json"}]
    http_client = Keyword.get(opts, :http_client)

    if http_client do
      url = Keyword.get(opts, :url, @linear_api_url)
      http_client.post(url, body, headers)
    else
      case Req.post(@linear_api_url, body: body, headers: headers) do
        {:ok, %{status: 200, body: resp_body}} -> {:ok, resp_body}
        {:ok, %{status: status}} -> {:error, {:http_error, status}}
        {:error, reason} -> {:error, reason}
      end
    end
  end

  defp normalize_issue(node) do
    labels =
      case get_in(node, ["labels", "nodes"]) do
        nil -> []
        label_nodes -> Enum.map(label_nodes, & &1["name"])
      end

    %{
      id: node["id"],
      identifier: node["identifier"],
      title: node["title"],
      description: node["description"] || "",
      priority: node["priority"],
      labels: labels
    }
  end

  defp state_to_linear_name(:started), do: "In Progress"
  defp state_to_linear_name(:completed), do: "Done"
  defp state_to_linear_name(:failed), do: "Todo"
  defp state_to_linear_name(:cancelled), do: "Cancelled"
  defp state_to_linear_name(other), do: to_string(other)
end
