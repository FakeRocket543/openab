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
   - `resolve_exec_nonzero_exit`：已修（`42f55609` 改用 `/usr/bin/false`），`cargo test -p openab-core resolve_exec` 6 passed。
   - Workspace clippy `-D warnings`：已透過 `m4z/openab-mcp-clippy` 修復（`7f7af411`），`cargo clippy --workspace -- -D warnings` OK。
   - Windows 跨平台檢查仍因本機缺少 `x86_64-w64-mingw32-gcc` 而無法執行。

3. **k3d image 的長期維護**
   - 本次是手動把編好的 `openab` binary patch 進既有 `openab-kiro:trixie` image。
   - 未來若要正式重建，應改以 `openab-fork/Dockerfile.unified` 編出 source image 後，再跑 `OAB-K3D/image/build.sh`。
## 2026-08-24 09:56 UTC 狀態更新

- `feat/janitor-hook-coordination` 已併入 OpenAB `main` @ `e813a8e4`：Janitor↔hooks 互鎖實作、M4-Review 修復、文件補充已推上 `forgejo`。
## 2026-08-25 04:28 UTC 狀態更新

- `m4z/openab-mcp-clippy` 已併入 OpenAB `main` @ `aa7bef7f`：修復 `openab-mcp` `-D warnings` 並解決 macOS `resolve_exec_nonzero_exit`；workspace clippy 全綠。
- `m4z/openab-mcp-clippy` 分支已刪除。
- `fix/shell-v1.1-r1` 已併入 cuta `main` @ `e37e2d8`：R1 W1/N1/N2/N3 修復完成，審查報告存於 `docs/reviews/m5-shell-v1.1-r2-review.md`。
## 2026-08-25 05:30 UTC 狀態更新

- `forgejo/fix/acp-agent-stderr-level` 已併入 OpenAB `main` @ `944d5500`：agent stderr 按自身 severity 轉發。
- `forgejo/kiro-v3-auth` 已併入 OpenAB `main` @ `944d5500`：re-land kiro-cli v3 auth on openab-acp。
- `fix-secrets-exec-test` 已刪除（`/bin/sh` 修復已被 `/usr/bin/false` 取代）。
- `feat/card-style-pack` 已併入 cuta `main` @ `0b70a71`：新增 `press-noir` / `aurora-dusk` 字卡風格與 `scripts/style_sheet.py`。
- `feat/shell-pushmode` 已併入 cuta `main` @ `d27972f`：Rust watcher emit `shell://mode`，nav 狀態燈 push/poll/unknown。
## 2026-08-25 05:55 UTC 狀態更新

- `feat/shell-timeline` 已併入 cuta `main` @ `dc5f4b4`：影片預覽 + 層時間軸 + 播放頭游標 + NLE 多軌檢視（視覺建議 #2）；pytest 242 passed、npm build ✅、clippy ✅
- `feat/shell-timeline` 已併入 cuta `main` @ `4e75516`：時間軸縮放拉長（OpenCut pattern：px 佈局＋縮放捲動＋自適應刻度＋拖曳 scrub）、npm build 無 a11y warning、pytest 242 passed

## 2026-08-25 07:15 UTC 狀態更新

- `feat/timeline-at-mode` 已併入 cuta `main` @ `b800c4a`：A/T hotkeys＋Jobs detail 時間軸滾輪、播放控制／follow playhead／seekable chips；補 `@tauri-apps/cli` devDependency。pytest 242 passed、npm build ✅、clippy `-D warnings` ✅
- `feat(formats)` 已併入 cuta `main` @ `1be9942`：輸出格式放寬——直式/橫式 × FHD/4K/social（5 內建 profile＋自訂 WxH@fps），manifest `format` 欄位驅動，無欄位時向後相容 fhd-vertical。pytest 256 passed；eversupp `video-fhd` 專案實跑 voice stage，base.mp4 實測 1920×1080@30。

## 2026-08-28 狀態更新

- 新增 `doc/20260828-central-skills-hooks-delegation.md`:對照官方水母概念圖盤點「中央技能供應 / hooks / 自動 delegation」的需求與現況。要點:①官方 hooks (pre_seed/pre_boot/configUrl) 本 fleet **零使用**,供應全靠 image 燒錄 (C5) + `sync-omp-profiles.sh` + `kubectl cp`;②`crates/openab-cp` delegation 協定 upstream 已實作但 ADR 仍 Proposed、無 `spawn_agent` LLM 工具,k3d 未部署——bot 間維持人類閘門 Discord handoff;③候選行動 P1=m4-free 試點 pre_seed (R2 bundle),**未批准未開工**。

## 2026-08-28 (二) 狀態更新

- 執行 omp fleet 三項修正並出自驗報告 `doc/20260828-omp-vision-advisor-fixes.md`:①vision role 改 `opencode-go/muse-spark-1.2-contributor`(主樹+m4-chimera+m4-free;glm-5.3 不收影像)②m4-chimera `heavy.md` 加 `advisor: true`(heavy 派工改由 Devin advisor 監審)③`.env` 權限查證後全 600 免改。subagent 審查抓到回歸:m4-free `enabledModels` 白名單缺 muse-spark → 已補並重驗(`--profile m4-free` 冒煙 OK)。omp 配置不在版控,repo 僅收報告。

## 2026-08-28 (三) 狀態更新

- sys101 k3d 同步:lcn-chimera-vite heavy.md 掛 `advisor: true`(OAB-K3D `c602030`,經 configmap patch + rollout);lcn-chimera 無 heavy 不變。**重要實證**:pod 端 `zai/glm-5.3-flash` (anthropic 端點) 運行時可收圖 (`omp -p @img` 實測),vision 維持 flash——與本地 `zai-coding` (coding 端點) flash 宣告 image 但運行拒收相反;本地 muse-spark 修正仍必要且已驗證。詳 `doc/20260828-omp-vision-advisor-fixes.md` §9。

## 2026-08-28 (四) 狀態更新

- 新增 `doc/20260828-chimera-plan-mode-lifecycle.md`:chimera 合體 (plan-yolo) 設計記錄——形態 2 (`--plan max --plan-yolo --plan-yolo-into flash:high`) **零 code 可上線** (只改 OAB-K3D configmap args);openab-fork 配合修改分三級:Tier 1 = Discord `/model` 指令 (仿既有 set_config_option 管線) + mode 狀態顯示;Tier 2 = ACP elicitation handler (connection.rs 仿 request_permission) 解鎖互動 plan 核准 + `session/set_mode` 解鎖同 session 重規劃。實證:openab-acp 僅處理 request_permission,無 elicitation/set_mode → 無人值守必須 yolo。待決策:Tier 0 上 vite 與否。

## 2026-08-28 (五) 狀態更新

- Tier 2 收官 (doc/20260828-tier2-implementation-report.md):**零 code 落地**。①`/models` `/agents` 上游已存在 (69118f3c,部署版已含);②elicitation.form 能力宣告實測使 omp ACP prompt 停擺 → 2.1 拒用,plan 核准 ACP headless 自動通過即正解;③`--plan` 旗標 ACP 下不換模型 (session jsonl per-turn 實證);④A2 引擎=`--plan-yolo-into`,已部署 vite (OAB-K3D 474ef51,pod e2e: max 規劃×3→flash 實作×8→pp.txt 建立)。lifecycle 文件已加 §7 修正。

## 2026-08-28 (六) 狀態更新

- k3d pods 模型端點統一:chimera/chimera-vite 的 `zai` provider 從 anthropic 端點 (按 token) 改為 **GLM Coding Plan** (`api.z.ai/api/coding/paas/v4`,與本機 zai-coding 同 key 同定義,provider 名保留故引用零改);flash 冒煙 OK。vision/vision2/fallback 同步改 muse-spark (coding 端點 flash 運行不收圖,上午實證)。chimera 兩 pod AGENTS.md 補 **Modes** 段 (Direct/Ralph/Interview,對齊 m4-z 條文);普查:本地 m4-z/m4-piswe 有、k3d Pi-Z 有 (中文變體)、MECRIVAIN 用 `MECRIVAIN_MODE=ralph` 環境變數、其餘 bot 無。OAB-K3D `5c94b94`;新 pod 6r5rt/2dttn 驗證 baseUrl/vision/Ralph/args 全數正確。

## 2026-08-28 (七) 狀態更新

- k3d fleet 紀律統一 (12 bots,OAB-K3D 值此 commit):①Anti-Loop 四條補齊 ecri-devin/lcn-muse/lcn-review/mecrivain;②Modes (Direct/Ralph/Interview) 補齊 8 隻 (mecrivain 保留其 MECRIVAIN_MODE 變體僅補 anti-loop);③chimera=重裝深度 (max, max_sessions 10→3) / vite=速度優先 (args 回 A1 flash:high 直答,深度靠 @heavy/關鍵字/@chimera),雙方 Identity 加定位行 (供人類路由,不含自動呼叫);④複審 12/12 一致,自動呼叫語句掃描 0;⑤Discord mentions-only 原本就 12/12。分工原則:使用者自行呼叫,bot 嚴守不搶答 (mentions-only + anti-loop)。

## 2026-08-28 (八) 狀態更新

- 本機追上 k3d 標準 (兩件):①m4-* bots 紀律統一——11 檔補齊 (m4-chimera/AGENTS + m4-review ×3 = anti-loop+Modes;m4-design ×3、m4-free ×3、m4-devin-swe/AGENTS = 補 Modes),複審 15/15 檔 anti+modes 全 ✓ (m4-z/m4-piswe 原有);openab-bots 工作區維持 untracked (含憑證不入版控)。②主樹 `~/.omp/agent` 加 `slow` role (zai-coding/glm-5.3:max) + `agents/heavy.md` (@slow/blocking/advisor:true)——`omp -p` 實測 task 派遣 heavy 成功回應。分工對照:m4-z ≈ k3d chimera (max)、m4-chimera ≈ k3d vite (flash+heavy),結構早已同構。
