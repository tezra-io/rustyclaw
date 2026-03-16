defmodule RustyclawOrchestrator.ToolSynthesis.StaticAnalyzer do
  @moduledoc """
  Allowlist-primary AST walker for synthesized tool validation.

  Parses Elixir source without compiling and walks every AST node.
  Only explicitly allowed modules and constructs pass validation.
  Everything else is rejected.
  """

  @max_lines 500
  @required_namespace RustyclawOrchestrator.Synth

  @allowed_modules [
    Enum,
    Map,
    List,
    String,
    Regex,
    Jason,
    Integer,
    Float,
    Tuple,
    Keyword,
    MapSet,
    Stream,
    Range,
    Access,
    URI,
    Base,
    Bitwise,
    Date,
    Time,
    DateTime,
    NaiveDateTime,
    RustyclawOrchestrator.ToolSynthesis.Composer
  ]

  @blocked_directives [:import, :use, :require]
  @blocked_defs [:defmacro, :defmacrop]

  @type error :: {:error, String.t()}
  @type result :: :ok | error()

  @doc """
  Validate source code for safety before compilation.

  Returns `:ok` if the source passes all checks, or `{:error, reason}`.
  """
  @spec validate(String.t()) :: result()
  def validate(source) when is_binary(source) do
    with :ok <- check_line_count(source),
         {:ok, ast} <- parse(source),
         :ok <- check_single_defmodule(ast),
         :ok <- check_namespace(ast) do
      walk_ast(ast)
    end
  end

  # --- Pre-parse checks ---

  defp check_line_count(source) do
    count = source |> String.split("\n") |> length()

    if count > @max_lines do
      {:error, "source exceeds #{@max_lines} line limit (#{count} lines)"}
    else
      :ok
    end
  end

  defp parse(source) do
    case Code.string_to_quoted(source) do
      {:ok, ast} -> {:ok, ast}
      {:error, {_meta, msg, token}} -> {:error, "parse error: #{msg} #{token}"}
    end
  end

  # --- Structural checks ---

  defp check_single_defmodule(ast) do
    count = count_defmodules(ast)

    case count do
      1 -> :ok
      0 -> {:error, "no defmodule found"}
      n -> {:error, "expected 1 defmodule, found #{n}"}
    end
  end

  defp count_defmodules(ast) do
    {_ast, count} =
      Macro.prewalk(ast, 0, fn
        {:defmodule, _meta, _args} = node, acc -> {node, acc + 1}
        node, acc -> {node, acc}
      end)

    count
  end

  defp check_namespace(ast) do
    case extract_module_name(ast) do
      {:ok, module} ->
        required_prefix = Module.split(@required_namespace)
        actual_prefix = module |> Module.split() |> Enum.take(length(required_prefix))

        if actual_prefix == required_prefix do
          :ok
        else
          {:error,
           "module must be in #{inspect(@required_namespace)} namespace, got #{inspect(module)}"}
        end

      :error ->
        {:error, "could not extract module name"}
    end
  end

  defp extract_module_name({:defmodule, _meta, [{:__aliases__, _, parts} | _]}) do
    {:ok, Module.concat(parts)}
  end

  defp extract_module_name({_form, _meta, args}) when is_list(args) do
    Enum.find_value(args, :error, &extract_module_name/1)
  end

  defp extract_module_name({_left, _right}) do
    :error
  end

  defp extract_module_name(_other), do: :error

  # --- AST walk ---

  defp walk_ast(ast) do
    Macro.prewalk(ast, fn node ->
      check_node!(node)
      node
    end)

    :ok
  catch
    :throw, {:blocked, reason} -> {:error, reason}
  end

  # Block import/use/require
  defp check_node!({directive, _meta, _args}) when directive in @blocked_directives do
    throw({:blocked, "#{directive} is not allowed"})
  end

  # Block defmacro/defmacrop
  defp check_node!({macro_def, _meta, _args}) when macro_def in @blocked_defs do
    throw({:blocked, "#{macro_def} is not allowed"})
  end

  # Block @on_load
  defp check_node!({:@, _meta, [{:on_load, _meta2, _args}]}) do
    throw({:blocked, "@on_load is not allowed"})
  end

  # Remote calls: Module.function(args)
  defp check_node!({{:., _dot_meta, [{:__aliases__, _, parts}, fun]}, _call_meta, _args}) do
    module = Module.concat(parts)
    check_remote_call!(module, fun)
  end

  # Erlang module calls: :atom.function(args)
  defp check_node!({{:., _dot_meta, [module, _fun]}, _call_meta, _args}) when is_atom(module) do
    throw({:blocked, "Erlang module call :#{module} is not allowed"})
  end

  # Block bare apply/3 calls
  defp check_node!({:apply, _meta, args}) when is_list(args) and length(args) == 3 do
    throw({:blocked, "apply/3 is not allowed"})
  end

  # Block spawn, spawn_link, spawn_monitor
  defp check_node!({spawn_fun, _meta, args})
       when spawn_fun in [:spawn, :spawn_link, :spawn_monitor] and is_list(args) do
    throw({:blocked, "#{spawn_fun} is not allowed"})
  end

  # Block send/2
  defp check_node!({:send, _meta, args}) when is_list(args) and length(args) == 2 do
    throw({:blocked, "send/2 is not allowed"})
  end

  defp check_node!(_node), do: :ok

  # --- Remote call validation ---

  @hard_blocked_modules [Code, EEx, Module, Process, Port, Node, System, File, Path]

  defp check_remote_call!(Logger, fun) when fun in [:debug, :info, :warning, :error], do: :ok

  defp check_remote_call!(String, :to_atom) do
    throw({:blocked, "String.to_atom/1 is not allowed (use String.to_existing_atom/1)"})
  end

  defp check_remote_call!(Kernel, :apply) do
    throw({:blocked, "Kernel.apply/3 is not allowed"})
  end

  defp check_remote_call!(Function, :capture) do
    throw({:blocked, "Function.capture/3 is not allowed"})
  end

  defp check_remote_call!(module, _fun) do
    cond do
      module in @hard_blocked_modules ->
        throw({:blocked, "#{inspect(module)} is not allowed"})

      module in @allowed_modules ->
        :ok

      true ->
        throw({:blocked, "module #{inspect(module)} is not in the allowlist"})
    end
  end
end
