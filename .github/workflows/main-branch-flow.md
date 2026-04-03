# Main Branch Delivery Flows

## Workflows

| File | Trigger | Purpose |
|------|---------|---------|
| `ci.yml` | PR to master | Rust build + test (ubuntu + macos-14). Required gate. |
| `ci-full.yml` | Manual dispatch | Extended matrix: linux-arm64, macos-13 (x86), Windows |
| `elixir-ci.yml` | Push/PR to master (elixir/ paths) | Elixir compile + credo + tests |
| `release.yml` | Push to master | Beta release: build artifacts for all targets, create prerelease tag |
| `promote-release.yml` | Manual dispatch | Promote beta → stable release (requires semver match in Cargo.toml) |

## Release Flow

1. Push to master → `release.yml` creates a `v{version}-beta.{run_number}` prerelease with binaries for all targets.
2. When ready for stable: bump version in `Cargo.toml`, run `promote-release.yml` with the version number → creates a stable `v{version}` release.

## PR Checks

`ci.yml` is the required gate for PRs. Both ubuntu and macos-14 (arm) builds must pass.
