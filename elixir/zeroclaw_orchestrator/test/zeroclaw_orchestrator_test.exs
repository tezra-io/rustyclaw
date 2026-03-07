defmodule ZeroclawOrchestratorTest do
  use ExUnit.Case

  describe "supervision tree" do
    test "application supervisor is running" do
      assert Process.whereis(ZeroclawOrchestrator.Supervisor) |> Process.alive?()
    end

    test "agent registry is running" do
      assert Process.whereis(ZeroclawOrchestrator.AgentRegistry) |> Process.alive?()
    end

    test "agent supervisor (DynamicSupervisor) is running" do
      assert Process.whereis(ZeroclawOrchestrator.AgentSupervisor) |> Process.alive?()
    end

    test "agent supervisor has no children initially" do
      children = DynamicSupervisor.which_children(ZeroclawOrchestrator.AgentSupervisor)
      assert children == []
    end
  end
end
