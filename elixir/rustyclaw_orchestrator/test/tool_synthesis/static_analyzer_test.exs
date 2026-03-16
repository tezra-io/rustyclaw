defmodule RustyclawOrchestrator.ToolSynthesis.StaticAnalyzerTest do
  use ExUnit.Case, async: true

  alias RustyclawOrchestrator.ToolSynthesis.StaticAnalyzer

  @valid_source """
  defmodule RustyclawOrchestrator.Synth.TestTool do
    @behaviour RustyclawOrchestrator.ToolSynthesis.SynthesizedTool

    def name, do: "test_tool"
    def description, do: "A test tool"
    def parameters_schema, do: %{}

    def execute(params) do
      result = Map.get(params, "input", "default")
      {:ok, String.upcase(result)}
    end
  end
  """

  describe "valid source" do
    test "accepts a well-formed synthesized tool" do
      assert :ok = StaticAnalyzer.validate(@valid_source)
    end

    test "accepts allowed module calls" do
      source = """
      defmodule RustyclawOrchestrator.Synth.AllowedModules do
        def name, do: "test"
        def description, do: "test"
        def parameters_schema, do: %{}

        def execute(params) do
          data = Map.get(params, "list", [])
          result = data
            |> Enum.map(&String.upcase/1)
            |> Enum.join(", ")
          {:ok, result}
        end
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end

    test "accepts pattern matching, case, cond, with, for" do
      source = """
      defmodule RustyclawOrchestrator.Synth.ControlFlow do
        def name, do: "cf"
        def description, do: "cf"
        def parameters_schema, do: %{}

        def execute(%{"mode" => mode} = params) do
          result = case mode do
            "upper" -> String.upcase(Map.get(params, "text", ""))
            "lower" -> String.downcase(Map.get(params, "text", ""))
            _ -> "unknown"
          end
          {:ok, result}
        end
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end

    test "accepts private functions" do
      source = """
      defmodule RustyclawOrchestrator.Synth.WithPrivate do
        def name, do: "p"
        def description, do: "p"
        def parameters_schema, do: %{}

        def execute(params) do
          {:ok, do_work(params)}
        end

        defp do_work(params), do: Map.get(params, "x", "none")
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end

    test "accepts Jason encoding" do
      source = """
      defmodule RustyclawOrchestrator.Synth.JsonTool do
        def name, do: "json"
        def description, do: "json"
        def parameters_schema, do: %{}

        def execute(params) do
          {:ok, Jason.encode!(params)}
        end
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end

    test "accepts Logger calls" do
      source = """
      defmodule RustyclawOrchestrator.Synth.WithLogger do
        def name, do: "log"
        def description, do: "log"
        def parameters_schema, do: %{}

        def execute(params) do
          Logger.info("executing")
          {:ok, "done"}
        end
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end

    test "accepts all allowed data modules" do
      source = """
      defmodule RustyclawOrchestrator.Synth.DataModules do
        def name, do: "data"
        def description, do: "data"
        def parameters_schema, do: %{}

        def execute(_params) do
          _list = List.flatten([[1], [2]])
          _kw = Keyword.get([a: 1], :a)
          _set = MapSet.new([1, 2, 3])
          _tup = Tuple.to_list({1, 2})
          _int = Integer.to_string(42)
          _flt = Float.round(3.14159, 2)
          _uri = URI.encode("hello world")
          _b64 = Base.encode64("hello")
          _date = Date.utc_today()
          _time = Time.utc_now()
          _dt = DateTime.utc_now()
          _ndt = NaiveDateTime.utc_now()
          {:ok, "all good"}
        end
      end
      """

      assert :ok = StaticAnalyzer.validate(source)
    end
  end

  describe "blocked constructs" do
    test "rejects import" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        import Enum
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "import"
    end

    test "rejects use" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        use GenServer
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "use"
    end

    test "rejects require" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        require Logger
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "require"
    end

    test "rejects defmacro" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        defmacro bad_macro(x), do: x
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "defmacro"
    end

    test "rejects defmacrop" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        defmacrop bad_macro(x), do: x
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "defmacrop"
    end

    test "rejects apply/3" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          apply(Enum, :map, [[1,2], &(&1 + 1)])
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "apply"
    end

    test "rejects Kernel.apply/3" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Kernel.apply(Enum, :map, [[1,2]])
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Kernel.apply"
    end

    test "rejects @on_load" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        @on_load :init
        def init, do: :ok
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "bad"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "@on_load"
    end

    test "rejects Erlang module calls" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          :os.cmd(~c"whoami")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Erlang module"
    end

    test "rejects :file module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          :file.read_file("/etc/passwd")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Erlang module"
    end

    test "rejects :ets module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          :ets.new(:stolen, [:set])
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Erlang module"
    end

    test "rejects String.to_atom/1" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(params) do
          atom = String.to_atom(params["key"])
          {:ok, Atom.to_string(atom)}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "String.to_atom"
    end

    test "rejects File module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          File.read!("/etc/passwd")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "File"
    end

    test "rejects System module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          System.cmd("rm", ["-rf", "/"])
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "System"
    end

    test "rejects Process module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Process.exit(self(), :kill)
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Process"
    end

    test "rejects Code module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Code.eval_string("1 + 1")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Code"
    end

    test "rejects Node module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Node.list()
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Node"
    end

    test "rejects Port module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Port.open({:spawn, "ls"}, [])
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Port"
    end

    test "rejects Module module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Module.create(Foo, quote(do: nil), __ENV__)
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Module"
    end

    test "rejects Function.capture/3" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          f = Function.capture(File, :read!, 1)
          f.("/etc/passwd")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Function.capture"
    end

    test "rejects non-allowlisted module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Agent.start(fn -> %{} end)
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "not in the allowlist"
    end

    test "rejects spawn" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          spawn(fn -> :ok end)
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "spawn"
    end

    test "rejects send/2" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          send(self(), :hello)
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "send"
    end

    test "rejects EEx module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          EEx.eval_string("<%= 1 + 1 %>")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "EEx"
    end

    test "rejects Path module" do
      source = """
      defmodule RustyclawOrchestrator.Synth.Bad do
        def name, do: "bad"
        def description, do: "bad"
        def parameters_schema, do: %{}
        def execute(_) do
          Path.expand("~")
          {:ok, "bad"}
        end
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "Path"
    end
  end

  describe "structural checks" do
    test "rejects source with no defmodule" do
      source = """
      def name, do: "orphan"
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "no defmodule"
    end

    test "rejects source with multiple defmodules" do
      source = """
      defmodule RustyclawOrchestrator.Synth.One do
        def name, do: "one"
      end

      defmodule RustyclawOrchestrator.Synth.Two do
        def name, do: "two"
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "expected 1 defmodule, found 2"
    end

    test "rejects module outside Synth namespace" do
      source = """
      defmodule RustyclawOrchestrator.Evil.Tool do
        def name, do: "evil"
        def description, do: "evil"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "evil"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "namespace"
    end

    test "rejects module in root namespace" do
      source = """
      defmodule EvilTool do
        def name, do: "evil"
        def description, do: "evil"
        def parameters_schema, do: %{}
        def execute(_), do: {:ok, "evil"}
      end
      """

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "namespace"
    end

    test "rejects source exceeding 500 lines" do
      lines = List.duplicate("  # comment line\n", 501)
      source = "defmodule RustyclawOrchestrator.Synth.Big do\n#{lines}end\n"

      assert {:error, msg} = StaticAnalyzer.validate(source)
      assert msg =~ "line limit"
    end

    test "rejects unparseable source" do
      assert {:error, msg} = StaticAnalyzer.validate("defmodule do {{{")
      assert msg =~ "parse error"
    end
  end
end
