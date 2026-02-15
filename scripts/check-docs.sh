#!/bin/bash
# Pre-commit hook: remind to update README.md and CLAUDE.md when significant files change.
# Checks if src/ structure changed (new modules, new dirs) but docs weren't updated.

STAGED=$(git diff --cached --name-only)

# Check if any new modules/directories were added under src/
NEW_MODULES=$(echo "$STAGED" | grep -E '^src/[^/]+/(mod\.rs|lib\.rs)$|^src/[^/]+\.rs$' | grep -v 'test')
CHANGED_CARGO=$(echo "$STAGED" | grep -q 'Cargo.toml' && echo "yes")
CHANGED_CONFIG=$(echo "$STAGED" | grep -q 'src/config/' && echo "yes")

# Check if docs are in the staged changes
DOCS_UPDATED=$(echo "$STAGED" | grep -qE '(README\.md|CLAUDE\.md)' && echo "yes")

if [[ -n "$NEW_MODULES" || -n "$CHANGED_CARGO" || -n "$CHANGED_CONFIG" ]] && [[ -z "$DOCS_UPDATED" ]]; then
    echo ""
    echo "⚠️  Significant changes detected but README.md/CLAUDE.md not updated:"
    [[ -n "$NEW_MODULES" ]] && echo "   New modules: $NEW_MODULES"
    [[ -n "$CHANGED_CARGO" ]] && echo "   Cargo.toml changed (new deps?)"
    [[ -n "$CHANGED_CONFIG" ]] && echo "   Config schema changed"
    echo ""
    echo "   Consider updating README.md and/or CLAUDE.md."
    echo "   To skip this check: git commit --no-verify"
    echo ""
    # Warning only, don't block the commit
fi

exit 0
