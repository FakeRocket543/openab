# M4-Z openab-mcp clippy / macOS exec test self-improvement report

> Date: 2026-08-25  
> Branch: `m4z/openab-mcp-clippy` → merged to `main` @ `e242eebe`

## Task
Resolve `cargo clippy --workspace -- -D warnings` failures and the macOS-only `resolve_exec_nonzero_exit` test failure.

## Findings and changes

| File | Change | Reason |
|---|---|---|
| `crates/openab-mcp/src/auth.rs` | `map.get(...).is_none()` → `!map.contains_key(...)`; `map.get(...).is_some()` → `map.contains_key(...)` | Clippy `unnecessary_get_then_check` |
| `crates/openab-mcp/src/mcp/runtime.rs` | `#[allow(clippy::await_holding_lock)]` on test | `std::sync::Mutex` held across `.await` only to serialize `std::env` mutations during concurrent tests; test-only guard |
| `crates/openab-mcp/src/native/gmail.rs` | removed unused `#[cfg(test)] fn with_api_base` | Clippy dead code |
| `crates/openab-core/src/secrets.rs` | test uses `/usr/bin/false` instead of `/bin/false` | macOS has no `/bin/false`; test now passes cross-platform |

## Verification

| Check | Command | Result |
|---|---|---|
| Workspace clippy | `cargo clippy --workspace -- -D warnings` | ✅ OK |
| `openab-mcp` tests | `cargo test -p openab-mcp` | ✅ 219 passed |
| `openab-core` tests | `cargo test -p openab-core resolve_exec` | ✅ 6 passed (incl. `resolve_exec_nonzero_exit`) |

## Conclusion

- Workspace clippy is now clean under `-D warnings`.
- `resolve_exec_nonzero_exit` no longer fails on macOS.
- No runtime behavior changes.
- Merged to `main` and pushed to `forgejo`.
