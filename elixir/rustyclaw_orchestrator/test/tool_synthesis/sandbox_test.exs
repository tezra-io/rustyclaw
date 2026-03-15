defmodule RustyclawOrchestrator.ToolSynthesis.SandboxTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.Sandbox

  # Compile test tool modules inline for sandbox testing

  defmodule GoodTool do
    def execute(%{"input" => input}), do: {:ok, String.upcase(input)}
    def execute(_), do: {:ok, "default"}
  end

  defmodule ErrorTool do
    def execute(_), do: {:error, "something went wrong"}
  end

  defmodule SlowTool do
    def execute(_) do
      Process.sleep(5_000)
      {:ok, "done"}
    end
  end

  defmodule BadReturnTool do
    def execute(_), do: :not_a_valid_return
  end

  defmodule BadReturnIntTool do
    def execute(_), do: {:ok, 42}
  end

  defmodule CrashTool do
    def execute(_), do: raise("boom")
  end

  defmodule LargeOutputTool do
    def execute(_) do
      # Generate output larger than 1MB
      output = String.duplicate("x", 2_000_000)
      {:ok, output}
    end
  end

  describe "execute/3" do
    test "executes a well-behaved tool and returns result" do
      assert {:ok, "HELLO"} = Sandbox.execute(GoodTool, %{"input" => "hello"})
    end

    test "returns error tuple from tool" do
      assert {:error, "something went wrong"} = Sandbox.execute(ErrorTool, %{})
    end

    test "enforces timeout" do
      assert {:error, msg} = Sandbox.execute(SlowTool, %{}, timeout: 100)
      assert msg =~ "timed out"
    end

    test "rejects invalid return type (bare atom)" do
      assert {:error, msg} = Sandbox.execute(BadReturnTool, %{})
      assert msg =~ "invalid tool output"
    end

    test "rejects invalid return type (non-string ok value)" do
      assert {:error, msg} = Sandbox.execute(BadReturnIntTool, %{})
      assert msg =~ "invalid tool output"
    end

    test "handles tool that raises an exception" do
      assert {:error, msg} = Sandbox.execute(CrashTool, %{})
      assert msg =~ "crashed"
    end

    test "truncates output exceeding 1MB" do
      assert {:ok, output} = Sandbox.execute(LargeOutputTool, %{})
      assert byte_size(output) == 1_048_576
    end

    test "passes params to tool" do
      assert {:ok, "WORLD"} = Sandbox.execute(GoodTool, %{"input" => "world"})
    end

    test "uses default params when none match" do
      assert {:ok, "default"} = Sandbox.execute(GoodTool, %{})
    end
  end
end
