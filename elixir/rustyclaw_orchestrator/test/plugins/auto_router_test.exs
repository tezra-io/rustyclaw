defmodule RustyclawOrchestrator.Plugins.AutoRouterTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.AutoRouter

  describe "route_task/1" do
    test "routes plugin:coding label to [:coding]" do
      assert AutoRouter.route_task(%{labels: ["plugin:coding"]}) == [:coding]
    end

    test "routes plugin:review label to [:review]" do
      assert AutoRouter.route_task(%{labels: ["plugin:review"]}) == [:review]
    end

    test "routes plugin:analysis label to [:analysis]" do
      assert AutoRouter.route_task(%{labels: ["plugin:analysis"]}) == [:analysis]
    end

    test "combines multiple plugin labels" do
      caps = AutoRouter.route_task(%{labels: ["plugin:coding", "plugin:review"]})
      assert :coding in caps
      assert :review in caps
    end

    test "defaults to [:coding] when no plugin labels" do
      assert AutoRouter.route_task(%{labels: ["bug", "urgent"]}) == [:coding]
    end

    test "defaults to [:coding] when labels are empty" do
      assert AutoRouter.route_task(%{labels: []}) == [:coding]
    end

    test "defaults to [:coding] when no labels key" do
      assert AutoRouter.route_task(%{}) == [:coding]
    end

    test "ignores non-plugin labels" do
      caps = AutoRouter.route_task(%{labels: ["bug", "plugin:analysis", "high-priority"]})
      assert caps == [:analysis]
    end

    test "deduplicates capabilities" do
      caps =
        AutoRouter.route_task(%{labels: ["plugin:coding", "plugin:coding", "plugin:coding"]})

      assert caps == [:coding]
    end
  end
end
