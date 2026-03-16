#!/usr/bin/env bash
#
# E2E Cleanup — removes temp files, kills lingering processes
#
set -euo pipefail

echo "Cleaning up E2E test artifacts..."

# Kill lingering rustyclaw processes from E2E tests
pkill -f "rustyclaw.*e2e" 2>/dev/null || true

# Clean temp workspaces
rm -rf /tmp/rustyclaw_e2e.* 2>/dev/null || true

# Clean old logs (keep last 7 days)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
find "$SCRIPT_DIR/logs" -name "*.log" -mtime +7 -delete 2>/dev/null || true

echo "Done."
