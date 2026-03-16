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

  @doc """
  Post a formatted progress comment to a Linear issue.

  Status must be one of: `:started`, `:in_progress`, `:quality_gate`, `:completed`, `:failed`.

  Progress map may include:
  - `:commits` — list of `%{hash: "abc123", message: "..."}` maps
  - `:quality_results` — list of `%{gate: "test", passed: true}` maps
  - `:duration_seconds` — elapsed time in seconds
  - `:error` — error message (for failed status)
  """
  @spec post_progress_comment(String.t(), atom(), map(), keyword()) :: :ok | {:error, term()}
  def post_progress_comment(issue_id, status, progress \\ %{}, opts \\ []) do
    body = format_progress_comment(status, progress)
    create_comment(issue_id, body, opts)
  end

  @doc """
  Update an issue to started state and post an assignment comment.
  """
  @spec update_issue_on_start(map(), keyword()) :: :ok | {:error, term()}
  def update_issue_on_start(issue, opts \\ []) do
    identifier = issue[:identifier] || issue["identifier"]
    issue_id = issue[:id] || issue["id"] || identifier

    with :ok <- update_issue_state(identifier, :started, opts) do
      post_progress_comment(issue_id, :started, %{}, opts)
    end
  end

  @doc """
  Update an issue to completed state and post a summary comment.

  Result map may include `:commits`, `:quality_results`, `:duration_seconds`.
  """
  @spec update_issue_on_complete(map(), map(), keyword()) :: :ok | {:error, term()}
  def update_issue_on_complete(issue, result \\ %{}, opts \\ []) do
    identifier = issue[:identifier] || issue["identifier"]
    issue_id = issue[:id] || issue["id"] || identifier

    with :ok <- update_issue_state(identifier, :completed, opts) do
      post_progress_comment(issue_id, :completed, result, opts)
    end
  end

  # --- Internals ---

  defp format_progress_comment(status, progress) do
    header = status_header(status)
    sections = build_sections(status, progress)
    Enum.join([header | sections], "\n\n")
  end

  defp status_header(:started), do: "## 🤖 Task Assigned to Worker\nStatus: **Started**"
  defp status_header(:in_progress), do: "## 🔄 Work In Progress\nStatus: **In Progress**"
  defp status_header(:quality_gate), do: "## 🔍 Quality Gate Running\nStatus: **Quality Gate**"
  defp status_header(:completed), do: "## ✅ Task Completed\nStatus: **Completed**"
  defp status_header(:failed), do: "## ❌ Task Failed\nStatus: **Failed**"
  defp status_header(other), do: "## Task Update\nStatus: **#{other}**"

  defp build_sections(status, progress) do
    sections = []

    sections = maybe_add_commits(sections, progress)
    sections = maybe_add_quality_results(sections, progress)
    sections = maybe_add_duration(sections, progress)
    sections = maybe_add_error(sections, status, progress)

    Enum.reverse(sections)
  end

  defp maybe_add_commits(sections, %{commits: commits}) when is_list(commits) and commits != [] do
    lines =
      Enum.map(commits, fn c ->
        hash = c[:hash] || c["hash"] || "?"
        msg = c[:message] || c["message"] || ""
        "- `#{String.slice(hash, 0, 7)}` #{msg}"
      end)

    ["### Commits\n#{Enum.join(lines, "\n")}" | sections]
  end

  defp maybe_add_commits(sections, _), do: sections

  defp maybe_add_quality_results(sections, %{quality_results: results})
       when is_list(results) and results != [] do
    lines =
      Enum.map(results, fn r ->
        gate = r[:gate] || r["gate"] || "unknown"
        passed = r[:passed] || r["passed"]
        icon = if passed, do: "✅", else: "❌"
        "- #{icon} #{gate}"
      end)

    ["### Quality Gates\n#{Enum.join(lines, "\n")}" | sections]
  end

  defp maybe_add_quality_results(sections, _), do: sections

  defp maybe_add_duration(sections, %{duration_seconds: seconds}) when is_number(seconds) do
    minutes = div(trunc(seconds), 60)
    secs = rem(trunc(seconds), 60)
    ["**Duration:** #{minutes}m #{secs}s" | sections]
  end

  defp maybe_add_duration(sections, _), do: sections

  defp maybe_add_error(sections, :failed, %{error: error}) when is_binary(error) do
    ["### Error\n```\n#{error}\n```" | sections]
  end

  defp maybe_add_error(sections, _, _), do: sections

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
