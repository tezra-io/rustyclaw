defmodule RustyclawOrchestratorTest do
  use ExUnit.Case

  describe "supervision tree" do
    test "application supervisor is running" do
      assert Process.whereis(RustyclawOrchestrator.Supervisor) |> Process.alive?()
    end

    test "agent registry is running" do
      assert Process.whereis(RustyclawOrchestrator.AgentRegistry) |> Process.alive?()
    end

    test "agent supervisor (DynamicSupervisor) is running" do
      assert Process.whereis(RustyclawOrchestrator.AgentSupervisor) |> Process.alive?()
    end

    test "agent supervisor has no children initially" do
      children = DynamicSupervisor.which_children(RustyclawOrchestrator.AgentSupervisor)
      assert children == []
    end
  end
end
