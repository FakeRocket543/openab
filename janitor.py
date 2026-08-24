#!/usr/bin/env python3
"""Devin sessions.db janitor — runs periodically or once inside a sidecar container.

Improvements over v1:
- PID-aware lock cleanup (only removes locks whose PID is dead)
- Coordination lock for VACUUM (prevents racing with agent writes)
- Pre-VACUUM WAL checkpoint to shrink WAL
- Graceful error handling (malformed DB → skip, don't crash)
- Configurable paths and intervals via environment variables
- One-shot mode (JANITOR_ONESHOT) for cronjobs / launch-on-demand
"""
import sqlite3
import fcntl
import glob
import json
import os
import signal
import sys
import time
from datetime import datetime
from pathlib import Path


def log(msg):
    print(f"[janitor] {datetime.now().isoformat()} {msg}", flush=True)


DB = os.environ.get("SESSIONS_DB", "/home/agent/.local/share/devin/cli/sessions.db")
LOCKS = os.path.join(os.path.dirname(DB), "session_locks")
TM = os.environ.get("OPENAB_THREAD_MAP", "/home/agent/.openab/thread_map.json")

try:
    INTERVAL = int(os.environ.get("JANITOR_INTERVAL", "21600"))  # 6h
    if INTERVAL <= 0:
        raise ValueError
except ValueError:
    log("WARNING: invalid JANITOR_INTERVAL, using default 21600")
    INTERVAL = 21600

try:
    VACUUM_THRESHOLD = int(os.environ.get("VACUUM_THRESHOLD_MB", "300"))
    if VACUUM_THRESHOLD <= 0:
        raise ValueError
except ValueError:
    log("WARNING: invalid VACUUM_THRESHOLD_MB, using default 300")
    VACUUM_THRESHOLD = 300
try:
    PASS_LOCK_WAIT_S = int(os.environ.get("JANITOR_LOCK_WAIT_S", "60"))
    if PASS_LOCK_WAIT_S < 0:
        raise ValueError
except ValueError:
    log("WARNING: invalid JANITOR_LOCK_WAIT_S, using default 60")
    PASS_LOCK_WAIT_S = 60

# Coordination lock — shared with lifecycle hooks (docs/hooks.md): pre_seed
# (restore) / pre_shutdown (backup) hold an exclusive flock on this file so
# this janitor can never checkpoint/VACUUM mid-backup or mid-restore. The
# agent can also flock it in shared mode before DB writes.
COORD_LOCK = os.environ.get("JANITOR_COORD_LOCK", os.path.join(os.path.dirname(DB), ".janitor.lock"))

_raw_oneshot = os.environ.get("JANITOR_ONESHOT", "").strip().lower()
if _raw_oneshot in ("1", "true", "yes", "on"):
    ONESHOT = True
elif _raw_oneshot in ("", "0", "false", "no", "off"):
    ONESHOT = False
else:
    log(f"WARNING: invalid JANITOR_ONESHOT={_raw_oneshot!r}, using default false")
    ONESHOT = False


def pid_alive(pid: int) -> bool:
    """Check if a process is still running."""
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def cleanup_stale_locks() -> bool:
    """Remove lock files only if their PID is dead. Return True on success, False on any failure."""
    removed = 0
    if not os.path.isdir(LOCKS):
        return True
    ok = True
    for f in glob.glob(os.path.join(LOCKS, "*.lock")):
        try:
            content = Path(f).read_text().strip()
            pid = int(content)
            if not pid_alive(pid):
                os.remove(f)
                removed += 1
                log(f"removed stale lock {os.path.basename(f)} (pid={pid} dead)")
            else:
                log(f"kept lock {os.path.basename(f)} (pid={pid} alive)")
        except (ValueError, OSError):
            # Lock file unreadable or not a PID — remove it
            try:
                os.remove(f)
                removed += 1
                log(f"removed unreadable lock {os.path.basename(f)}")
            except OSError as e:
                ok = False
                log(f"ERROR: failed to remove lock {os.path.basename(f)}: {e}")
    if removed:
        log(f"removed {removed} stale lock(s)")
    return ok


def acquire_coord_lock(timeout=60):
    """Acquire exclusive coordination lock (non-blocking with timeout).

    Returns the lock fd, or None if timed out.
    The agent can optionally flock this same file in shared mode before DB writes,
    which would cause us to wait here until it's done.
    """
    lock_fd = os.open(COORD_LOCK, os.O_CREAT | os.O_RDWR, 0o600)
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return lock_fd
        except (IOError, OSError):
            time.sleep(1)
    os.close(lock_fd)
    return None


def release_coord_lock(lock_fd):
    """Release coordination lock."""
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        os.close(lock_fd)
    except OSError:
        pass


def check_db_integrity(conn):
    """Quick integrity check before doing destructive operations."""
    try:
        result = conn.execute("PRAGMA integrity_check(1)").fetchone()
        return result[0] == "ok"
    except sqlite3.DatabaseError:
        return False


def cleanup() -> bool:
    """Run one cleanup pass. Return True on success, False if any step failed.

    The whole pass holds the coordination lock: lifecycle hooks (pre_seed
    restore / pre_shutdown backup, see docs/hooks.md) take the same exclusive
    flock, so they can never capture a torn DB/WAL pair or swap files under
    an in-flight VACUUM. A busy lock means a hook window is active — skip
    this pass gracefully (True: not a failure) and retry next interval.
    """
    lock_fd = acquire_coord_lock(timeout=PASS_LOCK_WAIT_S)
    if lock_fd is None:
        log("coord lock busy (pre_seed/pre_shutdown window?), skipping pass")
        return True
    try:
        return _cleanup_pass()
    finally:
        release_coord_lock(lock_fd)


def _cleanup_pass() -> bool:
    """Cleanup work — callers hold the coordination lock."""
    ok = True
    ok &= cleanup_stale_locks()

    if not os.path.exists(DB):
        log(f"sessions.db not found at {DB}, skipping")
        return False

    # Quick size check before opening
    size_mb = os.path.getsize(DB) // (1024 * 1024)
    wal_mb = 0
    wal_path = DB + "-wal"
    if os.path.exists(wal_path):
        wal_mb = os.path.getsize(wal_path) // (1024 * 1024)

    log(f"db={size_mb}MB wal={wal_mb}MB total={size_mb + wal_mb}MB")

    # Phase 1: Read-only cleanup (orphaned sessions) — no coord lock needed
    try:
        conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True, timeout=10)
        # Check integrity first
        if not check_db_integrity(conn):
            log("WARNING: DB integrity check failed, skipping cleanup to avoid further damage")
            conn.close()
            return False

        # Find orphaned sessions
        active = set()
        if os.path.exists(TM):
            try:
                with open(TM) as f:
                    tm = json.load(f)
                if not isinstance(tm, dict):
                    log("WARNING: thread map structure incorrect, expected dict")
                else:
                    active = set(str(v) for v in tm.values())
            except (json.JSONDecodeError, OSError, UnicodeDecodeError, AttributeError, TypeError) as e:
                log(f"WARNING: could not load thread map ({e})")

        conn.close()

        if active:
            # Reopen in read-write for deletes
            conn = sqlite3.connect(DB, timeout=10, isolation_level=None)
            conn.execute("PRAGMA busy_timeout=10000")
            all_sids = set(r[0] for r in conn.execute("SELECT id FROM sessions").fetchall())
            orphaned = all_sids - active
            if orphaned:
                for sid in orphaned:
                    conn.execute("DELETE FROM message_nodes WHERE session_id=?", (sid,))
                    conn.execute("DELETE FROM tool_call_state WHERE session_id=?", (sid,))
                    conn.execute("DELETE FROM sessions WHERE id=?", (sid,))
                log(f"deleted {len(orphaned)} orphaned session(s)")
            conn.close()
    except sqlite3.DatabaseError as e:
        log(f"ERROR during orphan cleanup: {e}")
        return False

    # Phase 2: WAL checkpoint + VACUUM — needs coordination
    if size_mb + wal_mb <= VACUUM_THRESHOLD:
        # Still do WAL checkpoint even if we don't VACUUM
        try:
            conn = sqlite3.connect(DB, timeout=30, isolation_level=None)
            conn.execute("PRAGMA busy_timeout=30000")
            r = conn.execute("PRAGMA wal_checkpoint(PASSIVE)").fetchone()
            log(f"wal_checkpoint(PASSIVE): busy={r[0]} log={r[1]} ckpt={r[2]}")
            conn.close()
            return ok
        except sqlite3.DatabaseError as e:
            log(f"WAL checkpoint failed: {e}")
            return False

    log(f"db={size_mb}MB > {VACUUM_THRESHOLD}MB threshold, attempting VACUUM")

    # Coordination lock is held for the whole pass (see cleanup()) — the
    # agent's shared-flock writes are excluded via SQLite busy_timeout.
    try:
        conn = sqlite3.connect(DB, timeout=30, isolation_level=None)
        conn.execute("PRAGMA busy_timeout=30000")

        # Recheck integrity under lock
        if not check_db_integrity(conn):
            log("WARNING: DB integrity check failed under lock, skipping VACUUM")
            conn.close()
            return False

        # WAL checkpoint first to minimize WAL size
        r = conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
        log(f"pre-vacuum wal_checkpoint(TRUNCATE): busy={r[0]} log={r[1]} ckpt={r[2]}")

        # VACUUM
        before_mb = os.path.getsize(DB) // (1024 * 1024)
        conn.execute("VACUUM")
        after_mb = os.path.getsize(DB) // (1024 * 1024)
        log(f"VACUUM ok: {before_mb}MB -> {after_mb}MB")

        conn.close()
    except sqlite3.DatabaseError as e:
        log(f"VACUUM failed: {e}")
        return False
    return ok


# Graceful shutdown
def handle_signal(sig, frame):
    log(f"received signal {sig}, shutting down")
    sys.exit(0)


signal.signal(signal.SIGTERM, handle_signal)
signal.signal(signal.SIGINT, handle_signal)

log(f"started interval={INTERVAL}s vacuum_threshold={VACUUM_THRESHOLD}MB coord_lock={COORD_LOCK} oneshot={ONESHOT}")

while True:
    if not ONESHOT:
        time.sleep(INTERVAL)
    ok = False
    try:
        ok = cleanup()
    except Exception as e:
        log(f"ERROR: {e}")
    if ONESHOT:
        if ok:
            log("oneshot complete, exiting")
            sys.exit(0)
        log("oneshot failed, exiting")
        sys.exit(1)
