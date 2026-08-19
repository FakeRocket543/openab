# Devin ACP stale session lock 清理筆記

> 適用對象：後續維護此系統的 subagents / 開發者

## 背景

Devin ACP agent 在 OpenAB 中頻繁發生 `session/load` 失敗，錯誤訊息為 `already open in another process`，導致 OpenAB 每次都退回 `session/new`，造成 cold start 比例過高、反應極慢。根據調查，主要元兇是 Devin 程序或容器異常結束後遺留在 `~/.local/share/devin/cli/session_locks/<session_id>.lock` 的 stale lock 檔。

## 本次修改總覽

### 1. openab-fork（核心程式碼）

新增一個向後相容的 `[pool]` 設定 `session_lock_cleanup`（預設 `false`）。啟用後，當 `session/load` 失敗且錯誤訊息包含 `already open in another process` 時，OpenAB 會：

1. 讀取該 session 對應的 `.lock` 檔，取得裡面的 PID。
2. 在 Unix 系統上以 `libc::kill(pid, 0)` 檢查該 PID 是否仍存活。
3. 若 `kill()` 返回 -1 且 `errno == ESRCH`，代表進程已消失，判斷為 stale lock。
4. 刪除該 lock 檔，並**重試一次** `session/load`。

修改檔案：

- `crates/openab-core/src/config.rs`
  - `PoolConfig` 新增 `session_lock_cleanup: bool`，預設 `false`。
- `crates/openab-core/src/acp/pool.rs`
  - `SessionPool` 新增欄位與 `set_session_lock_cleanup()`。
  - 在 `AcpConnection` 初始化後把設定傳下去。
- `crates/openab-acp/src/connection.rs`
  - `AcpConnection` 新增 `clean_stale_session_locks` 欄位。
  - 新增 `devin_session_lock_path()` 與 `maybe_remove_stale_session_lock()`。
  - 重構 `session_load()` 為 `try_session_load()`，實作 lock 清理與重試邏輯。
- `src/main.rs`
  - 從 `cfg.pool.session_lock_cleanup` 讀取並設定給 `SessionPool`。
- `config.toml.example` / `docs/config-reference.md`
  - 補上設定說明。

Commit：`8ec006f fix(acp): recover from stale devin session locks`

### 2. 本機 m4-devin-swe

- 編譯 release `openab` 並更換 `~/.cargo/bin/openab`。
- `openab-bots/m4-devin-swe/config.toml` 在 `[pool]` 加入 `session_lock_cleanup = true`。
- 以 `launchctl kickstart -k gui/501/com.openab.m4-devin-swe` 重啟。

### 3. k3d（OAB-K3D / sys101）

- 在 sys101 用 Docker 編譯新版 Linux `openab` binary。
- 以 `openab-kiro:trixie` 為基底製作新 image，只覆蓋 `/usr/local/bin/openab`。
- 對 `openab-kiro:trixie-lcn-swe`、`trixie-lcn-visual`、`trixie-lcn-review`、`trixie-ecri-devin` 等 tag 做 `k3d image import`。
- 更新 `values.yaml` 與 `k8s/live/openab-*-configmap.yaml`，為所有 Devin ACP agent 啟用 `session_lock_cleanup = true`。
- 重啟 deployments：
  - `openab-ecri-devin`
  - `openab-lcn-swe`
  - `openab-lcn-visual`
  - `openab-lcn-review`

### 4. Devin credential 持久化

- 解密並更新 `OAB-K3D/secrets/devin-creds.yaml.age`，在 `config.json` 寫入 `"auto_update": false`，避免未來 `helm upgrade` 又把 auto_update 打開。
- OAB-K3D commit：`c0580ba`

## 驗證狀態

- `cargo fmt`、`cargo clippy -p openab-core -p openab-acp -- -D warnings` 通過。
- `cargo test -p openab-acp` 全過。
- k3d pods 重啟後皆 `Running`。
- pod 內 `openab --version` 為 `0.10.0`。
- pod 內 `md5sum /usr/local/bin/openab` 與新 build 的 image 一致。
- pod 內 `/etc/openab/config.toml` 已含 `session_lock_cleanup = true`。

## 已知限制 / 後續建議

1. **`sessions.db` 過大仍未處理**
   - 本機 `~/.local/share/devin/cli/sessions.db` 約 1.8GB，k3d PVC 上約 369MB。
   - 建議在低峰時段停機執行：
     ```bash
     sqlite3 sessions.db 'VACUUM;'
     sqlite3 sessions.db 'PRAGMA wal_checkpoint(TRUNCATE);'
     ```
   - k3d 需在對應 pod 的 PVC 路徑上執行。

2. **測試結果備註**
   - `cargo test -p openab-core` 有一筆既有的 `secrets::tests::resolve_exec_nonzero_exit` 在 macOS 上失敗，與本次修改無關。
   - Windows 跨平台檢查因本機缺少 `x86_64-w64-mingw32-gcc` 而無法執行。

3. **k3d image 的長期維護**
   - 本次是手動把編好的 `openab` binary patch 進既有 `openab-kiro:trixie` image。
   - 未來若要正式重建，應改以 `openab-fork/Dockerfile.unified` 編出 source image 後，再跑 `OAB-K3D/image/build.sh`。
