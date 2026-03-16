defmodule RustyclawOrchestrator.Plugins.LinearIntegrationTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.LinearIntegration

  defmodule MockHTTPClient do
    def post(url, body, _headers) do
      decoded = Jason.decode!(body)

      cond do
        String.contains?(decoded["query"], "issues(") ->
          handle_fetch_issues(url, decoded)

        String.contains?(decoded["query"], "issueUpdate") ->
          handle_update_issue(url, decoded)

        String.contains?(decoded["query"], "commentCreate") ->
          handle_create_comment(url, decoded)

        true ->
          {:error, :unknown_query}
      end
    end

    defp handle_fetch_issues(_url, _decoded) do
      {:ok,
       %{
         "data" => %{
           "issues" => %{
             "nodes" => [
               %{
                 "id" => "issue-1",
                 "identifier" => "TEZ-100",
                 "title" => "Fix the bug",
                 "description" => "Something is broken",
                 "priority" => 1,
                 "labels" => %{"nodes" => [%{"name" => "bug"}, %{"name" => "urgent"}]}
               },
               %{
                 "id" => "issue-2",
                 "identifier" => "TEZ-101",
                 "title" => "Add feature",
                 "description" => nil,
                 "priority" => 2,
                 "labels" => %{"nodes" => []}
               }
             ]
           }
         }
       }}
    end

    defp handle_update_issue(_url, _decoded) do
      {:ok, %{"data" => %{"issueUpdate" => %{"success" => true}}}}
    end

    defp handle_create_comment(_url, _decoded) do
      {:ok, %{"data" => %{"commentCreate" => %{"success" => true}}}}
    end
  end

  defmodule ErrorHTTPClient do
    def post(_url, _body, _headers) do
      {:ok, %{"errors" => [%{"message" => "Not authorized"}]}}
    end
  end

  defmodule NetworkErrorHTTPClient do
    def post(_url, _body, _headers) do
      {:error, :timeout}
    end
  end

  describe "fetch_todo_issues/2" do
    test "fetches and normalizes issues from Linear" do
      {:ok, issues} =
        LinearIntegration.fetch_todo_issues("TEZ",
          api_key: "test-key",
          http_client: MockHTTPClient
        )

      assert length(issues) == 2

      [first, second] = issues
      assert first.identifier == "TEZ-100"
      assert first.title == "Fix the bug"
      assert first.description == "Something is broken"
      assert first.priority == 1
      assert first.labels == ["bug", "urgent"]

      assert second.identifier == "TEZ-101"
      assert second.description == ""
      assert second.labels == []
    end

    test "returns error when API key is missing" do
      result = LinearIntegration.fetch_todo_issues("TEZ", api_key: nil)
      assert {:error, :missing_api_key} = result
    end

    test "returns graphql errors" do
      result =
        LinearIntegration.fetch_todo_issues("TEZ",
          api_key: "test-key",
          http_client: ErrorHTTPClient
        )

      assert {:error, {:graphql_errors, [%{"message" => "Not authorized"}]}} = result
    end

    test "returns network errors" do
      result =
        LinearIntegration.fetch_todo_issues("TEZ",
          api_key: "test-key",
          http_client: NetworkErrorHTTPClient
        )

      assert {:error, :timeout} = result
    end
  end

  describe "update_issue_state/3" do
    test "updates issue state successfully" do
      result =
        LinearIntegration.update_issue_state("TEZ-100", :completed,
          api_key: "test-key",
          http_client: MockHTTPClient
        )

      assert :ok = result
    end

    test "handles started state" do
      result =
        LinearIntegration.update_issue_state("TEZ-100", :started,
          api_key: "test-key",
          http_client: MockHTTPClient
        )

      assert :ok = result
    end

    test "handles failed state" do
      result =
        LinearIntegration.update_issue_state("TEZ-100", :failed,
          api_key: "test-key",
          http_client: MockHTTPClient
        )

      assert :ok = result
    end

    test "returns error when API key is missing" do
      result = LinearIntegration.update_issue_state("TEZ-100", :completed, api_key: nil)
      assert {:error, :missing_api_key} = result
    end
  end

  describe "create_comment/3" do
    test "creates comment successfully" do
      result =
        LinearIntegration.create_comment("issue-1", "Build passed!",
          api_key: "test-key",
          http_client: MockHTTPClient
        )

      assert :ok = result
    end

    test "returns error when API key is missing" do
      result = LinearIntegration.create_comment("issue-1", "comment", api_key: nil)
      assert {:error, :missing_api_key} = result
    end

    test "handles graphql errors" do
      result =
        LinearIntegration.create_comment("issue-1", "comment",
          api_key: "test-key",
          http_client: ErrorHTTPClient
        )

      assert {:error, {:graphql_errors, _}} = result
    end
  end
end
