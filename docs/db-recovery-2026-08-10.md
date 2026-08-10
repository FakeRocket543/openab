# DB Recovery: sessions.db malformed (2026-08-10)

## Root Cause

Janitor v1 (`janitor.py`) had a race condition:
1. Unconditionally deleted all lock files (including live agent locks)
2. Ran `VACUUM` while agent was actively writing to `sessions.db`
3. SQLite B-tree corruption → `database disk image is malformed`

## Symptoms

```
[janitor] 2026-08-09T16:07:10 removed 5 stale lock(s)
[janitor] 2026-08-09T16:07:27 ERROR: database disk image is malformed
```

Agent pod stays Running (2/2) but stops responding to messages.

## Recovery Steps

```bash
# 1. Copy original DB + WAL out of the pod
kubectl cp default/<pod>:/home/agent/.local/share/devin/cli/sessions.db /tmp/sessions_orig.db -c openab
kubectl cp default/<pod>:/home/agent/.local/share/devin/cli/sessions.db-wal /tmp/sessions_orig.db-wal -c openab

# 2. Use sqlite3 .recover to extract readable data
sqlite3 /tmp/sessions_orig.db ".recover" > /tmp/sessions_recovered.sql

# 3. Filter reserved table names and rebuild
grep -v "sqlite_sequence" /tmp/sessions_recovered.sql | sqlite3 /tmp/sessions_fresh.db

# 4. Verify integrity
sqlite3 /tmp/sessions_fresh.db "PRAGMA integrity_check;"

# 5. Deploy to pod
kubectl cp /tmp/sessions_fresh.db default/<pod>:/tmp/sessions_fresh.db -c openab
kubectl exec <pod> -c openab -- sh -c '
  cd /home/agent/.local/share/devin/cli
  mv sessions.db sessions.db.corrupted
  cp /tmp/sessions_fresh.db sessions.db
'

# 6. Restart
kubectl rollout restart deployment/openab-<agent>
```

## Recovery Results

| Table | Before | After | % |
|-------|--------|-------|---|
| sessions | 24 | 24 | 100% |
| message_nodes | 545,818 | 40,311 | 7.4% |
| tool_call_state | 7,383 | 7,333 | 99.3% |

## Fix

See `janitor.py` (v2) — PID-aware lock cleanup + coordination lock.
