#!/bin/bash
# sync-omp-profiles.sh — 同步主樹 ~/.omp/agent 的憑證到各 bot profile
# 用法: sync-omp-profiles.sh {models|auth|mcp|all|status}
#   models — 輪替 coding key 後：主樹 models.yml → 4 profiles
#   auth   — devin re-auth 後：.env + agent.db auth_* 表 → 4 profiles（不動 profile 自身 sessions）
#   mcp    — gbrain token 換新後：主樹 mcp.json → 4 profiles
#   status — 只顯示各處指紋/計數，不改任何東西
set -eu
PROFILES="m4-z m4-free m4-review m4-piswe"
MAIN=~/.omp/agent
cd "$(dirname "$0")"

fp() { python3 -c "
import re,sys,hashlib
try:
    k=re.search(r'apiKey:\s*(\S+)',open('$1').read())
    print(hashlib.sha256(k.group(1).encode()).hexdigest()[:8] if k else 'NO-KEY')
except FileNotFoundError: print('NO-FILE')"; }

authcount() { sqlite3 "$1" "SELECT COUNT(*) FROM auth_credentials" 2>/dev/null || echo ERR; }

mcptok() { python3 -c "
import json,hashlib
try:
    t=json.load(open('$1'))['mcpServers']['gbrain']['headers']['Authorization'].split(' ',1)[1]
    print(hashlib.sha256(t.encode()).hexdigest()[:8])
except Exception: print('ERR')" ; }

cmd="${1:-status}"

case "$cmd" in
status)
  echo "main:    models=$(fp $MAIN/models.yml) auth=$(authcount $MAIN/agent.db) gbrain=$(mcptok $MAIN/mcp.json)"
  for p in $PROFILES; do
    D=~/.omp/profiles/$p/agent
    echo "$p: models=$(fp $D/models.yml) auth=$(authcount $D/agent.db) gbrain=$(mcptok $D/mcp.json)"
  done
  ;;
models)
  for p in $PROFILES; do
    D=~/.omp/profiles/$p/agent
    cp "$MAIN/models.yml" "$D/models.yml" && chmod 600 "$D/models.yml"
    echo "$p: models.yml synced ($(fp $D/models.yml))"
  done
  ;;
mcp)
  for p in $PROFILES; do
    D=~/.omp/profiles/$p/agent
    cp "$MAIN/mcp.json" "$D/mcp.json" && chmod 600 "$D/mcp.json"
    echo "$p: mcp.json synced ($(mcptok $D/mcp.json))"
  done
  ;;
auth)
  TABLES="auth_credentials auth_credential_blocks auth_credential_refresh_leases auth_change_revision auth_credential_block_mirror_guard"
  for p in $PROFILES; do
    D=~/.omp/profiles/$p/agent
    cp "$MAIN/.env" "$D/.env" && chmod 600 "$D/.env"
    # 先停該 bot 正在跑的 omp session（若有），避免 db 鎖
    LAUNCHED=""
    if launchctl list 2>/dev/null | grep -q "com.openab.$p"; then
      launchctl kickstart -k gui/$(id -u)/com.openab.$p && LAUNCHED=" (+bot restarted)"
    fi
    sqlite3 "$D/agent.db" <<SQL
PRAGMA busy_timeout=5000;
ATTACH '$MAIN/agent.db' AS src;
$(for t in $TABLES; do echo "DELETE FROM $t; INSERT INTO $t SELECT * FROM src.$t;"; done)
DETACH src;
SQL
    echo "$p: auth synced ($(authcount $D/agent.db) rows)$LAUNCHED"
  done
  ;;
all)
  "$0" models && "$0" mcp && "$0" auth && "$0" status
  ;;
*)
  echo "usage: $0 {models|auth|mcp|all|status}" >&2; exit 64
  ;;
esac
