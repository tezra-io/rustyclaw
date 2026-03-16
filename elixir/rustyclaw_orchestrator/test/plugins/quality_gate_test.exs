defmodule RustyclawOrchestrator.Plugins.QualityGateTest do
  use ExUnit.Case

  alias RustyclawOrchestrator.Plugins.QualityGate

  describe "run/2" do
    test "returns pass with empty gates" do
      assert {:pass, []} = QualityGate.run(%{}, [])
    end

    test "passes when command succeeds" do
      gates = [%{name: "echo", command: "echo 'hello'"}]
      assert {:pass, [output]} = QualityGate.run(%{}, gates)
      assert output.name == "echo"
      assert output.status == :pass
      assert String.contains?(output.output, "hello")
    end

    test "fails when command exits non-zero" do
      gates = [%{name: "fail", command: "exit 1"}]
      assert {:fail, "fail", output} = QualityGate.run(%{}, gates)
      assert output.status == :fail
      assert String.contains?(output.output, "exit code 1")
    end

    test "runs multiple gates in sequence" do
      gates = [
        %{name: "first", command: "echo 'first'"},
        %{name: "second", command: "echo 'second'"}
      ]

      assert {:pass, outputs} = QualityGate.run(%{}, gates)
      assert length(outputs) == 2
      assert Enum.at(outputs, 0).name == "first"
      assert Enum.at(outputs, 1).name == "second"
    end

    test "stops at first failure" do
      gates = [
        %{name: "pass", command: "echo 'ok'"},
        %{name: "fail", command: "exit 42"},
        %{name: "never", command: "echo 'should not run'"}
      ]

      assert {:fail, "fail", output} = QualityGate.run(%{}, gates)
      assert output.status == :fail
      assert String.contains?(output.output, "exit code 42")
    end

    test "handles command that produces output before failing" do
      gates = [%{name: "verbose_fail", command: "echo 'error details' && exit 2"}]
      assert {:fail, "verbose_fail", output} = QualityGate.run(%{}, gates)
      assert String.contains?(output.output, "error details")
    end

    test "handles timeout" do
      gates = [%{name: "slow", command: "sleep 10", timeout: 200}]
      assert {:fail, "slow", output} = QualityGate.run(%{}, gates)
      assert String.contains?(output.output, "timeout")
    end

    test "supports cwd option" do
      gates = [%{name: "pwd", command: "pwd", cwd: "/tmp"}]
      assert {:pass, [output]} = QualityGate.run(%{}, gates)
      # On macOS, /tmp is a symlink to /private/tmp
      assert String.contains?(output.output, "tmp")
    end

    test "captures stderr in output" do
      gates = [%{name: "stderr", command: "echo 'err' >&2 && exit 1"}]
      assert {:fail, "stderr", output} = QualityGate.run(%{}, gates)
      assert String.contains?(output.output, "err")
    end

    test "accepts string keys in gate config" do
      gates = [%{"name" => "str_keys", "command" => "echo 'ok'"}]
      assert {:pass, [output]} = QualityGate.run(%{}, gates)
      assert output.name == "str_keys"
    end
  end
end
