# .githooks

This directory contains Git hooks for the substrate project.

## Available hooks

| Hook | Checks performed |
|---|---|
| `pre-commit` | `cargo fmt --check`, `cargo clippy -D warnings`, `spec validate --lane fast` |

## Opt-in installation

Git hooks are **opt-in**. Run this once after cloning:

```bash
git config core.hooksPath .githooks
```

To verify the hook is active:

```bash
git config --get core.hooksPath
# expected output: .githooks
```

## Requirements

- `just` — task runner (installed via `mise`, see `mise.toml`)
- `spec` — spec framework CLI from `~/bin/spec` (optional; hook warns and continues if absent)

## Uninstall

```bash
git config --unset core.hooksPath
```

This resets Git to use the default `.git/hooks` directory (empty by default).
