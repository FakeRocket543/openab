#!/usr/bin/env python3
"""Devin sessions.db janitor — runs periodically inside a sidecar container.

Improvements over v1:
- PID-aware lock cleanup (only removes locks whose PID is dead)
- Coordination lock for VACUUM (prevents racing with agent writes)
- Pre-VACUUM WAL checkpoint to shrink WAL
- Graceful error handling (malformed DB → skip, don't crash)
"""
import sqlite3, os, glob, time, json, fcntl, signal, sys
from datetime import datetime
from pathlib import Path

DB = os.environ.get("SESSIONS_DB", "/home/agent/.local/share/devin/cli/sessions.db")
LOCKS = os.path.join(os.path.dirname(DB), "session_locks")
TM = "/home/agent/.openab/thread_map.json"
INTERVAL = int(os.environ.get("JANITOR_INTERVAL", "21600"))  # 6h
VACUUM_THRESHOLD = int(os.environ.get("VACUUM_THRESHOLD_MB", "300"))
# Coordination lock — agent can optionally flock this before DB writes
COORD_LOCK = os.environ.get("JANITOR_COORD_LOCK", os.path.join(os.path.dirname(DB), ".janitor.lock"))


def log(msg):
    print(f"[janitor] {datetime.now().isoformat()} {msg}", flush=True)


def pid_alive(pid: int) -> bool:
    """Check if a process is still running."""
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def cleanup_stale_locks():
    """Remove lock files only if their PID is dead."""
    removed = 0
    if not os.path.isdir(LOCKS):
        return
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
            except OSError:
                pass
    if removed:
        log(f"removed {removed} stale lock(s)")


def acquire_coord_lock(timeout=60):
    """Acquire exclusive coordination lock (non-blocking with timeout).

    Returns the lock fd, or None if timed out.
    The agent can optionally flock this same file in shared mode before DB writes,
    which would cause us to wait here until it's done.
    """
    lock_fd = os.open(COORD_LOCK, os.O_CREAT | os.O_RDWR, 0o666)
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


def cleanup():
    cleanup_stale_locks()

    if not os.path.exists(DB):
        log(f"sessions.db not found at {DB}, skipping")
        return

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
            return

        # Find orphaned sessions
        active = set()
        if os.path.exists(TM):
            try:
                with open(TM) as f:
                    tm = json.load(f)
                active = set(str(v) for v in tm.values())
            except (json.JSONDecodeError, OSError):
                pass

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
        return

    # Phase 2: WAL checkpoint + VACUUM — needs coordination
    if size_mb + wal_mb <= VACUUM_THRESHOLD:
        # Still do WAL checkpoint even if we don't VACUUM
        try:
            conn = sqlite3.connect(DB, timeout=30, isolation_level=None)
            conn.execute("PRAGMA busy_timeout=30000")
            r = conn.execute("PRAGMA wal_checkpoint(PASSIVE)").fetchone()
            log(f"wal_checkpoint(PASSIVE): busy={r[0]} log={r[1]} ckpt={r[2]}")
            conn.close()
        except sqlite3.DatabaseError as e:
            log(f"WAL checkpoint failed: {e}")
        return

    log(f"db={size_mb}MB > {VACUUM_THRESHOLD}MB threshold, attempting VACUUM with coord lock")

    # Acquire coordination lock — wait up to 120s for agent to finish current writes
    lock_fd = acquire_coord_lock(timeout=120)
    if lock_fd is None:
        log("VACUUM skipped: could not acquire coord lock within 120s")
        return

    try:
        conn = sqlite3.connect(DB, timeout=30, isolation_level=None)
        conn.execute("PRAGMA busy_timeout=30000")

        # Recheck integrity under lock
        if not check_db_integrity(conn):
            log("WARNING: DB integrity check failed under lock, skipping VACUUM")
            conn.close()
            return

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
    finally:
        release_coord_lock(lock_fd)


log(f"started interval={INTERVAL}s vacuum_threshold={VACUUM_THRESHOLD}MB coord_lock={COORD_LOCK}")

# Graceful shutdown
def handle_signal(sig, frame):
    log(f"received signal {sig}, shutting down")
    sys.exit(0)

signal.signal(signal.SIGTERM, handle_signal)
signal.signal(signal.SIGINT, handle_signal)

while True:
    time.sleep(INTERVAL)
    try:
        cleanup()
    except Exception as e:
        log(f"ERROR: {e}")
