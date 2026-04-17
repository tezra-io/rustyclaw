#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ELIXIR_DIR="$REPO_ROOT/elixir/rustyclaw_orchestrator"

print_help() {
  cat <<'EOF'
RustyClaw Unified Build

Usage: ./dev/build.sh <command>

Commands:
  build         Build Rust + Elixir
  check         Format + lint + test both (full validation gate)
  rust          Build Rust only
  elixir        Build Elixir only
  clean         Remove all build artifacts (Rust target/, Elixir _build/ + deps/)
  clean-rust    Remove Rust build artifacts only
  clean-elixir  Remove Elixir build artifacts only
EOF
}

build_rust() {
  echo "==> Building Rust..."
  cd "$REPO_ROOT"
  cargo build
  echo "==> Rust build OK"
}

build_elixir() {
  echo "==> Building Elixir..."
  cd "$ELIXIR_DIR"
  mix deps.get
  mix compile --warnings-as-errors
  echo "==> Elixir build OK"
}

check_rust() {
  echo "==> Checking Rust (fmt + clippy + test)..."
  cd "$REPO_ROOT"
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test --quiet
  echo "==> Rust checks passed"
}

check_elixir() {
  echo "==> Checking Elixir (format + compile + credo + test)..."
  cd "$ELIXIR_DIR"
  mix deps.get --quiet
  mix format --check-formatted
  mix compile --warnings-as-errors
  mix credo --strict
  mix test --quiet
  echo "==> Elixir checks passed"
}

clean_rust() {
  echo "==> Cleaning Rust artifacts..."
  cd "$REPO_ROOT"
  cargo clean
  echo "==> Rust clean OK"
}

clean_elixir() {
  echo "==> Cleaning Elixir artifacts..."
  cd "$ELIXIR_DIR"
  rm -rf _build deps
  echo "==> Elixir clean OK"
}

if [ $# -lt 1 ]; then
  print_help
  exit 1
fi

case "$1" in
  build)
    build_rust
    build_elixir
    ;;
  check)
    check_rust
    check_elixir
    ;;
  rust)
    build_rust
    ;;
  elixir)
    build_elixir
    ;;
  clean)
    clean_rust
    clean_elixir
    ;;
  clean-rust)
    clean_rust
    ;;
  clean-elixir)
    clean_elixir
    ;;
  *)
    print_help
    exit 1
    ;;
esac
