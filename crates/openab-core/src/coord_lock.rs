//! Coordination lock between lifecycle hooks and the sessions.db janitor.
//!
//! The janitor sidecar (`janitor.py`) periodically runs WAL checkpoint /
//! VACUUM / orphan cleanup on the agent's `sessions.db`. `pre_seed` (restore)
//! and `pre_shutdown` (backup) touch the same files from the main container.
//! Without an interlock, a backup can capture a torn WAL/DB pair or a restore
//! can swap the DB underneath an in-flight VACUUM — the exact corruption class
//! documented in `docs/db-recovery-2026-08-10.md`.
//!
//! Mechanism: advisory `flock(2)` on a lock file, held for the whole
//! hook window. Both sides lock the *same path*:
//!
//! - janitor: env `JANITOR_COORD_LOCK`, default `<dirname($SESSIONS_DB)>/.janitor.lock`
//! - openab: `[hooks] coordination_lock`, default `$HOME/.local/share/devin/cli/.janitor.lock`
//!
//! flock is kernel-atomic (no check-then-act window) and is released
//! automatically if the holding process dies — no stale sentinel files.
//!
//! The lock file lives inside `$HOME`, i.e. inside the backup/restore tree.
//! `pre_seed` therefore **skips restoring this exact path**: a rename over a
//! held lock file would swap the inode out from under the holder and silently
//! break mutual exclusion mid-extraction.

use std::path::PathBuf;
use std::time::Duration;

use tracing::info;

/// Default lock path relative to `$HOME` — matches janitor.py's default
/// (`dirname(SESSIONS_DB)/.janitor.lock` with the default SESSIONS_DB).
pub const DEFAULT_LOCK_RELPATH: &str = ".local/share/devin/cli/.janitor.lock";

/// Default time a hook waits for the janitor to finish its current pass.
const DEFAULT_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone)]
pub struct CoordLockSpec {
    pub path: PathBuf,
    pub timeout: Duration,
}

impl CoordLockSpec {
    /// Resolve the spec from `[hooks]` config. Returns `None` when no hooks
    /// are configured (nothing to interlock). Defaults preserve previous
    /// behavior: uncontended lock = no-op for existing deployments.
    pub fn resolve(hooks: &crate::config::HooksConfig) -> Option<Self> {
        if !hooks.any_configured() {
            return None;
        }
        let path = hooks
            .coordination_lock
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(default_lock_path);
        let timeout = Duration::from_secs(
            hooks
                .coordination_lock_timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        );
        Some(Self { path, timeout })
    }
}
pub fn default_lock_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/agent"))
        .join(DEFAULT_LOCK_RELPATH)
}

/// Acquire the exclusive coordination lock, waiting up to `spec.timeout`.
///
/// Errors when the lock cannot be taken in time (janitor pass still running)
/// or on an unexpected I/O failure. Callers MUST NOT proceed with DB-adjacent
/// work after an error — racing is what this lock exists to prevent.
pub async fn acquire(spec: &CoordLockSpec) -> anyhow::Result<CoordLockGuard> {
    let guard = acquire_impl(spec).await?;
    info!(path = %spec.path.display(), "coordination lock acquired");
    Ok(guard)
}

// --------------------------------------------------------------- unix impl
#[cfg(unix)]
mod imp {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    pub struct CoordLockGuard {
        file: Option<File>,
        path: PathBuf,
    }

    impl CoordLockGuard {
        /// A guard that holds nothing — used when no coordination is
        /// configured (hooks absent) so call sites avoid Option juggling.
        pub fn noop() -> Self {
            Self {
                file: None,
                path: PathBuf::new(),
            }
        }
    }

    impl Drop for CoordLockGuard {
        fn drop(&mut self) {
            if let Some(file) = &self.file {
                // flock is released on close; LOCK_UN makes intent explicit.
                unsafe {
                    libc::flock(file.as_raw_fd(), libc::LOCK_UN);
                }
                info!(path = %self.path.display(), "coordination lock released");
            }
        }
    }

    pub async fn acquire_impl(spec: &CoordLockSpec) -> anyhow::Result<CoordLockGuard> {
        // Fresh stateless hosts may not have the DB directory yet; the lock
        // must be acquirable before pre_seed extracts anything (docs/hooks.md).
        if let Some(parent) = spec.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false) // lock file content is never used — never clobber
            .mode(0o600)
            .open(&spec.path)?;
        let deadline = tokio::time::Instant::now() + spec.timeout;
        loop {
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(CoordLockGuard {
                    file: Some(file),
                    path: spec.path.clone(),
                });
            }
            let err = std::io::Error::last_os_error();
            let code = err.raw_os_error();
            let would_block = code == Some(libc::EWOULDBLOCK)
                || code == Some(libc::EAGAIN)
                || code == Some(libc::EINTR);
            if !would_block {
                return Err(anyhow::anyhow!(
                    "flock({}) failed: {}",
                    spec.path.display(),
                    err
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow::anyhow!(
                    "timed out after {}s waiting for coordination lock {} — janitor pass still \
                     running; refusing to race (see docs/hooks.md)",
                    spec.timeout.as_secs(),
                    spec.path.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

// ----------------------------------------------------- non-unix stub
// Hooks fail fast at startup on non-Unix platforms (HooksConfig::
// ensure_platform_supported), so this guard is unreachable in practice;
// it exists so the crate cross-compiles.
#[cfg(not(unix))]
mod imp {
    use super::*;

    pub struct CoordLockGuard;

    impl CoordLockGuard {
        pub fn noop() -> Self {
            Self
        }
    }

    pub async fn acquire_impl(_spec: &CoordLockSpec) -> anyhow::Result<CoordLockGuard> {
        Ok(CoordLockGuard)
    }
}

use imp::acquire_impl;
pub use imp::CoordLockGuard;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec(path: &std::path::Path, timeout_s: u64) -> CoordLockSpec {
        CoordLockSpec {
            path: path.to_path_buf(),
            timeout: Duration::from_secs(timeout_s),
        }
    }

    #[tokio::test]
    async fn lock_is_exclusive_then_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(".janitor.lock");
        let s = spec(&lock, 0);

        let g1 = acquire(&s).await.expect("first acquire");
        // Zero timeout + held lock → must fail, not race.
        assert!(acquire(&s).await.is_err(), "second acquire must fail");

        drop(g1);
        // After drop the lock is free again.
        acquire(&s)
            .await
            .expect("re-acquire after drop must succeed");
    }

    #[tokio::test]
    async fn timeout_error_mentions_janitor() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(".janitor.lock");
        let _held = acquire(&spec(&lock, 0)).await.unwrap();
        let err = acquire(&spec(&lock, 0))
            .await
            .err()
            .expect("second acquire must fail");
        assert!(err.to_string().contains("janitor"), "err: {err}");
    }

    #[tokio::test]
    async fn acquire_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("deep/nested/dir/.janitor.lock");
        let g = acquire(&spec(&lock, 0))
            .await
            .expect("acquire must create missing parent directories");
        drop(g);
    }
    #[test]
    fn resolve_returns_none_without_hooks() {
        let hooks = crate::config::HooksConfig::default();
        assert!(CoordLockSpec::resolve(&hooks).is_none());
    }
    #[test]
    fn resolve_uses_explicit_path_over_default() {
        let hook = crate::config::HookConfig {
            script: Some("/bin/true".into()),
            inline: None,
            url: None,
            sha256: None,
            timeout_seconds: 60,
            on_failure: crate::config::OnFailure::Warn,
        };
        let mut hooks = crate::config::HooksConfig {
            pre_boot: Some(hook),
            ..Default::default()
        };
        hooks.coordination_lock = Some("/custom/lock".into());
        hooks.coordination_lock_timeout_seconds = Some(7);
        let spec = CoordLockSpec::resolve(&hooks).expect("spec with hooks");
        assert_eq!(spec.path, PathBuf::from("/custom/lock"));
        assert_eq!(spec.timeout, Duration::from_secs(7));
    }
}
