defmodule RustyclawOrchestrator.ToolSynthesis.RegistryTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.ToolSynthesis.Registry

  # Test tool module compiled inline
  defmodule FakeTool do
    def name, do: "fake_tool"
    def description, do: "A fake tool for testing"
    def parameters_schema, do: %{"type" => "object", "properties" => %{}}
    def execute(_), do: {:ok, "done"}
  end

  defmodule AnotherTool do
    def name, do: "another_tool"
    def description, do: "Another fake tool"
    def parameters_schema, do: %{}
    def execute(_), do: {:ok, "also done"}
  end

  setup do
    Registry.clear()
    :ok
  end

  describe "register/3" do
    test "registers a new tool" do
      assert :ok = Registry.register("fake_tool", FakeTool)
    end

    test "rejects duplicate registration" do
      :ok = Registry.register("fake_tool", FakeTool)
      assert {:error, :already_exists} = Registry.register("fake_tool", FakeTool)
    end

    test "stores author_agent from opts" do
      :ok = Registry.register("fake_tool", FakeTool, author_agent: "research-agent")
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.author_agent == "research-agent"
    end

    test "defaults to probation status" do
      :ok = Registry.register("fake_tool", FakeTool)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.status == :probation
    end

    test "accepts custom initial status" do
      :ok = Registry.register("fake_tool", FakeTool, status: :promoted)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.status == :promoted
    end

    test "captures description and schema from module" do
      :ok = Registry.register("fake_tool", FakeTool)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.description == "A fake tool for testing"
      assert entry.parameters_schema == %{"type" => "object", "properties" => %{}}
    end

    test "initializes metrics to zero" do
      :ok = Registry.register("fake_tool", FakeTool)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.invocation_count == 0
      assert entry.success_count == 0
      assert entry.total_latency_ms == 0
    end

    test "sets created_at timestamp" do
      :ok = Registry.register("fake_tool", FakeTool)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert %DateTime{} = entry.created_at
    end
  end

  describe "lookup/1" do
    test "returns entry for registered tool" do
      :ok = Registry.register("fake_tool", FakeTool)
      assert {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.name == "fake_tool"
      assert entry.module == FakeTool
    end

    test "returns not_found for unregistered tool" do
      assert {:error, :not_found} = Registry.lookup("nonexistent")
    end
  end

  describe "update_metrics/3" do
    test "increments invocation count on success" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_metrics("fake_tool", true, 50)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.invocation_count == 1
      assert entry.success_count == 1
      assert entry.total_latency_ms == 50
    end

    test "increments invocation count on failure" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_metrics("fake_tool", false, 100)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.invocation_count == 1
      assert entry.success_count == 0
      assert entry.total_latency_ms == 100
    end

    test "accumulates across multiple invocations" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_metrics("fake_tool", true, 10)
      :ok = Registry.update_metrics("fake_tool", true, 20)
      :ok = Registry.update_metrics("fake_tool", false, 30)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.invocation_count == 3
      assert entry.success_count == 2
      assert entry.total_latency_ms == 60
    end

    test "returns not_found for unregistered tool" do
      assert {:error, :not_found} = Registry.update_metrics("ghost", true, 10)
    end
  end

  describe "update_status/2" do
    test "updates tool status" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_status("fake_tool", :promoted)
      {:ok, entry} = Registry.lookup("fake_tool")
      assert entry.status == :promoted
    end

    test "supports all status values" do
      :ok = Registry.register("fake_tool", FakeTool)

      for status <- [:probation, :promoted, :suspended, :deprecated] do
        :ok = Registry.update_status("fake_tool", status)
        {:ok, entry} = Registry.lookup("fake_tool")
        assert entry.status == status
      end
    end

    test "returns not_found for unregistered tool" do
      assert {:error, :not_found} = Registry.update_status("ghost", :promoted)
    end
  end

  describe "unload/1" do
    test "removes tool from registry" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.unload("fake_tool")
      assert {:error, :not_found} = Registry.lookup("fake_tool")
    end

    test "unloading nonexistent tool is a no-op" do
      assert :ok = Registry.unload("nonexistent")
    end
  end

  describe "list/1" do
    test "returns all registered tools" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.register("another_tool", AnotherTool)
      entries = Registry.list()
      names = Enum.map(entries, & &1.name) |> Enum.sort()
      assert names == ["another_tool", "fake_tool"]
    end

    test "returns empty list when no tools registered" do
      assert Registry.list() == []
    end

    test "filters by status" do
      :ok = Registry.register("tool_a", FakeTool)
      :ok = Registry.register("tool_b", AnotherTool, status: :promoted)
      probation = Registry.list(status: :probation)
      promoted = Registry.list(status: :promoted)
      assert length(probation) == 1
      assert hd(probation).name == "tool_a"
      assert length(promoted) == 1
      assert hd(promoted).name == "tool_b"
    end
  end

  describe "success_rate/1" do
    test "returns nil when no invocations" do
      :ok = Registry.register("fake_tool", FakeTool)
      assert nil == Registry.success_rate("fake_tool")
    end

    test "calculates rate correctly" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_metrics("fake_tool", true, 10)
      :ok = Registry.update_metrics("fake_tool", true, 10)
      :ok = Registry.update_metrics("fake_tool", false, 10)
      assert_in_delta Registry.success_rate("fake_tool"), 0.667, 0.01
    end

    test "returns not_found for unregistered tool" do
      assert {:error, :not_found} = Registry.success_rate("ghost")
    end
  end

  describe "avg_latency/1" do
    test "returns nil when no invocations" do
      :ok = Registry.register("fake_tool", FakeTool)
      assert nil == Registry.avg_latency("fake_tool")
    end

    test "calculates average correctly" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.update_metrics("fake_tool", true, 10)
      :ok = Registry.update_metrics("fake_tool", true, 30)
      assert_in_delta Registry.avg_latency("fake_tool"), 20.0, 0.01
    end

    test "returns not_found for unregistered tool" do
      assert {:error, :not_found} = Registry.avg_latency("ghost")
    end
  end

  describe "clear/0" do
    test "removes all entries" do
      :ok = Registry.register("fake_tool", FakeTool)
      :ok = Registry.register("another_tool", AnotherTool)
      :ok = Registry.clear()
      assert Registry.list() == []
    end
  end
end
