defmodule ZeroclawOrchestrator.RustBridgeTest do
  use ExUnit.Case

  alias ZeroclawOrchestrator.RustBridge

  # RustBridge is started by the application supervisor.

  describe "base_url/0" do
    test "returns configured base URL" do
      url = RustBridge.base_url()
      assert is_binary(url)
      assert String.starts_with?(url, "http")
    end
  end

  describe "health_check/0 (no Rust core running)" do
    test "returns error when Rust core is not reachable" do
      assert {:error, _reason} = RustBridge.health_check()
    end
  end

  describe "run_task/3 (no Rust core running)" do
    @tag timeout: 30_000
    test "returns max_retries_exceeded when Rust core is unreachable" do
      assert {:error, :max_retries_exceeded} = RustBridge.run_task("test-agent", "do something")
    end
  end
end
