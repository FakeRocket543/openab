# M4-Free — `origin/main..main` 未推送範圍審查報告（Ralph Mode）

**Date:** 2026-08-26
**Scope:** `origin/main..main`，44 commits（head `45300ec8` → 修復後 `42e01b77`），+6,288/−1,796，64 files
**Reviewer:** M4-Free（ox-alpha ACP）
**Method:** Ralph mode — 4 個平行 review subagents（KiroAuth / JanitorLock / DevinLockStderr / MiscFixes）＋ 中央 E2E（fmt/clippy/test/py/bash）＋ fix round ＋ 重跑全測

---

## E2E 結果

| Gate | 初次 | 修復後 |
|---|---|---|
| `cargo fmt --all --check` | **FAIL**（connection.rs ×4 hunks） | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | PASS |
| `cargo test --workspace` | PASS（全數通過） | PASS（含新回歸測試） |
| `python3 -m py_compile janitor.py` | PASS | PASS |
| `bash -n sync-omp-profiles.sh` | —（shellcheck 未安裝） | PASS |
| `cargo check --target x86_64-pc-windows-gnu` | N/A | **受阻**：缺 `x86_64-w64-mingw32-gcc`（既有環境限制） |

初次 fmt 檢查的 exit code 曾被 `| tail` 管線吞掉（誤報 PASS）；以 `set -o pipefail` 重跑取得權威結果。fmt 違規為純格式重排、零語義變更。

---

## Blockers（2，皆已修復）

### B1. kiro-cli whoami 子行程繼承完整父環境（憑證外洩規則違反）
- `crates/openab-acp/src/kiro_auth.rs:301`（修復前行號）：fallback spawn 只有 `.env("PATH", …)`，無 `.env_clear()`；對照 connection.rs agent spawn 明確 `cmd.env_clear()`，證明父行程帶有 bot 憑證。
- **修復：** `8f9fa9f5` — env_clear 後重建最小 HOME/PATH baseline。

### B2. Coordination lock 在全新狀態下開檔失敗
- `crates/openab-core/src/coord_lock.rs:117`：`OpenOptions::create(true)` 不建父目錄；`janitor.py:117` 的 `os.open` 同樣炸 `FileNotFoundError`。docs/hooks.md 自己描述的 stateless host 流程（家目錄清空 → pre_seed 還原）在第一次啟動就會卡死：lock 必須在 extraction 前取得，但 DB 目錄尚不存在。
- **修復：** `92dff51c` — Rust 側 `create_dir_all(parent)`；Python 側 `os.makedirs(dirname, exist_ok=True)`；新增回歸測試 `acquire_creates_missing_parent_dirs`（已驗證執行通過）。

---

## Warnings 已修復（5）

| # | 位置 | 問題 | 修復 |
|---|---|---|---|
| W1 | `coord_lock.rs:174` | 非 Unix stub `async fn … -> Result<_> {}` 回傳 `()`，E0308，跨平台編譯必炸 | `92dff51c` 改回 `Ok(CoordLockGuard)` |
| W2 | `janitor.py:76` | `pid_alive` 把 `PermissionError`（EPERM＝程序存在但他人 UID）當死亡 → janitor 可刪掉活著的 holder 的 session lock | `92dff51c` 加 `except PermissionError: return True` |
| W3 | `pre_seed.rs:214` | `strip_skipped` 用 `exists()`（跟隨 symlink）；壞 symlink 條目在 lock 路徑上可存活，`move_recursive` 換掉被 flock 的 inode | `92dff51c` 改 `symlink_metadata`（lstat 語義） |
| W4 | `sync-omp-profiles.sh:63` | auth 同步裸奔 `DELETE+INSERT` 無交易；中途失敗留空憑證表 | `3c497772` 包進 `BEGIN IMMEDIATE…COMMIT`，DETACH 移到 COMMIT 後 |
| W5 | `sync-omp-profiles.sh:61` | restart 先於 db sync → 重啟載不到新憑證；且 `launchctl list \| grep -q` 前綴子字串比對可能踢錯服務 | `3c497772` 改 sync 完才 kickstart；awk 精確比對 label 欄 |

---

## Warnings 已記錄、建議 follow-up（未動代碼，設計級決策）

1. **kiro token refresh 無同步**（`kiro_auth.rs:262）：並發 `get_access_token` 可雙重 OIDC refresh（rotation 下第二次使用可能作廢第一個 refresh token）；共用 sqlite 無 `busy_timeout`，寫入撞鎖被 `let _ =` 靜默丟棄。**建議：** process-wide mutex 序列化 expire→refresh→save 並鎖內重讀；兩處 connection 設 2s busy_timeout；save 失敗改記 log。
2. **whoami fallback 無界且早退路徑跳過 cooldown**（`:310/:312-318`）：`.output()` 無 timeout 可無限掛住 `_kiro/auth/getAccessToken`；DB 開不起來時 cooldown 不生效，形成 docstring 明言要避免的 retry storm。**建議：** tokio::process + 15s timeout；所有早退路徑先寫 `LAST_REFRESH_FAIL_MS`。
3. **coord_spec 取自 secret 展開前的 config**（`src/main.rs:453`）：`[hooks]` 內含 secret 引用時 pre_seed/pre_shutdown 會互鎖不同路徑。**建議：** resolve 移到 re-parse 之後。
4. **stderr 迴圈遇 invalid UTF-8 永久停止且無診斷**（openab-acp/connection.rs:596，pre-existing）：一行髒位元組即靜默結束轉送。**建議：** InvalidData 時 drain 到下一個 `\n` 並 warn 後 continue。
5. **stale-lock 清理只驗 PID 存在性、check-then-remove 非原子**（connection.rs:696）：torn read / fork-daemon / 共用 $HOME TOCTOU 三個窄窗。**建議：** unlink 前重讀確認 pid 未變，或 rename aside。
6. **sync script 無新鮮度防護**（`:42`）：profile 端較新的憑證可被主樹舊快照無條件覆蓋。屬運維語義決策——若要保留單向推送意圖，至少加 `SYNC_FORCE=1` 才覆蓋差異端。
7. **slash `/agents` 行為**：本次已補回 dual-category（見下），但 kiro 類後端的 mode 分頁 UI 建議補一支 e2e 手動驗證。

## Nits（記錄不修）

- STS 過期變體未納入 auth-failure matcher（`expiredtokenexception` 等，connection.rs:270）。
- kiro 測試兩處並發改 env 無共享鎖（kiro_auth.rs:384，註解宣稱唯一性已過期）。
- kiro auth deps 未放 `[target.'cfg(unix)'.dependencies]`（Cargo.toml:20，Unix 外白編譯成 dead code）。
- sync script：`set -eu` 缺 pipefail（目前唯一 pipeline 位於 if 條件，無活體 bug）；python one-liner 以字串內插路徑（應改 argv 傳參）。
- session_id 未消毒即拼進 lock 路徑（defense-in-depth，agent 本就有使用者權限）。
- "already open" 以英文錯誤文字字串比對、無覆蓋測試（connection.rs:1044）。
- janitor.py:55-58/110-115 註解描述從未實作的 shared-flock agent 協定；docs/hooks.md 未提 pre_seed 取不到鎖會中止啟動。

---

## 本次新增修復（超出 reviewer 報告、reviewer nit 直接吸收）

- `3c497772` 順手修了 launchctl 精確比對（MiscFixesReviewer nit）。
- `42e01b77` **discord `/agents` dual-category**：ae21db4d 只認 `Some("mode")`，但 openab-acp fallback（protocol.rs:187-194）合成的是 category `"agent"`，text gateway 特意雙認（gateway.rs:416-18）。修復：`build_config_select`(1582) 與 `build_config_components`(1678) 兩處 find 接受 `mode` 請求下的 `"agent"` fallback，與 gateway 行為一致，omp 現部署不受影響。
- `740d236e` fmt（E2E 發現）。

## Verified OK（摘要）

- **Kiro auth：** token 不落 log/error（逐點核對）；auth callback 是 stdio JSON-RPC in-band（無 listener 可加固）；缺 token 即回 -32001/-32603 有測試；matcher 刻意窄化且有反例測試；lifecycle 斷鏈→pool 重建閉環有測試；region 注入防護 + RFC3339 offset 保留皆有測試；non-Unix 回 JSON-RPC error 不 hang。
- **Coord lock：** 兩側同路徑真 flock（非 O_EXCL）、整段 critical section 覆蓋、flock 核心 atomic、kill -9 自動釋放、timeout 全部有界（60s/180s）、fail-closed/fail-visible 政策正確、停用時 serde default 還原舊行為、VACUUM threshold fallback 正常。
- **Devin lock/stderr：** 清理僅在「agent 自報 already-open ∧ kill=ESRCH」雙條件觸發，EPERM/zombie/sleep 全保守保留；level 分類五級有測試、未知行 fallback WARN 可見、控制字元先消毒；`session_lock_cleanup` 預設 false 完整還原舊行為，docs/example 一致；實機 lock 檔（6-byte `<pid>\n`）佐證解析假設。
- **Misc：** omp 的 configOptions 確實只有 `mode`（ACP schema const）；25-option 上限由分頁機制保護；clippy 修復全為測試碼/dead code 且 merge `e242eebe` 零夾帶（diff = 0 行驗證）；script 指紋輸出不含明文 secret、chmod 600 到位、status 唯讀、idempotent。

---

## Push-readiness 結論

**可推送。** 2 blockers + 5 warnings 已修復並全綠重驗（fmt/clippy/test/py/bash）。剩餘為文件化的 follow-up 設計項（上表 7 項），無一阻擋 push。Windows cross-check 因本機缺 mingw toolchain 無法執行（與前輪審查相同限制）；建議 CI 端補跑。

Commits：`8f9fa9f5` `740d236e` `92dff51c` `3c497772` `42e01b77`

## Self Improvements（下輪流程改進）

1. **Gate 前置：** fmt/clippy 應在 spawn reviewers 前先跑——本輪 fmt 遲到造成 reviewer 引用行號漂移風險與二次編譯。
2. **管線紀律：** E2E exit code 兩度被 `\| tail` 吞掉；一律 `set -o pipefail` 或分離 capture。
3. **Schema 一致性：** 各 reviewer output schema 統一要求 severity/confidence 欄位（本輪 Kiro 給了 confidence，Janitor 沒給，彙整需人肉對齊）。
4. **跨 slice 去重：** EPERM 語義同時出現在 Janitor 與 Devin 兩份報告；整合階段可用一次交叉比對自動合併同根因 findings。
5. **Script 測試化：** sync-omp-profiles.sh 僅 bash -n 太弱；建議 temp-HOME fixture 冒煙測試（假 launchctl/sqlite3 stub）納入 test/data。
6. **Windows gate：** 本機裝 mingw 或改依賴 CI matrix，別讓 cross-check 永遠掛在「環境限制」。

---

*Generated with M4-Free (ox-alpha) · ralph mode: review → subagent fan-out → central E2E → fix → re-test → report.*
