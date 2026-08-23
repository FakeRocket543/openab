# 本機 OpenAB bots 與 sys101 k3d 環境隔離方案（方案 A：omp --profile）

日期：2026-08-24
狀態：**已批准（A），尚未實作** — 說「開工」才動手
背景事故：2026-08-24，停用 opencode-go key 後全本機 bots 連帶 401，根因為共享 `~/.omp/agent/`。

## 問題

本機 6 bots（m4-z/free/review/design/piswe/devin-swe，LaunchAgent 啟動）全部讀同一份 omp 狀態：

- `~/.omp/agent/config.yml` — modelRoles.default（曾指 opencode-go → 單點炸全隊）
- `~/.omp/agent/models.yml` — provider zai-coding + key
- `~/.omp/agent/mcp.json` — gbrain MCP 接線（2026-08-23 建）
- `~/.omp/agent/{agent.db,sessions/,logs/}` — 狀態與歷史

只有 working_dir（AGENTS.md）與 LaunchAgent env 是每 bot 分開的。
sys101 k3d 10 pods 天生隔離（每 pod 獨立 PVC + netns），本機無此結構。

## 決策

採 **方案 A：omp `--profile`**。已實測（2026-08-24）：

- `omp acp --profile <name>` 支援（ACP initialize OK，omp 18.0.1）
- profile 樹落在 `~/.omp/profiles/<name>/agent/`：config.yml、models.yml、mcp.json、sessions、agent.db、logs 完全獨立
- bare model name 仍會 fuzzy-match 到任何 provider 目錄（含 opencode-go）→ 一律用全名 `zai-coding/glm-5.3`

否決 B（隔離 HOME）：ssh key/git 全要重播種，破壞本機工具鏈。
否決 C（本機容器化）：自廢本機工具優勢（ffmpeg/brew/M4 Max），image 維護線翻倍。

## 實作計畫（開工後執行）

1. 為 6 bots 各建 profile：`~/.omp/profiles/<m4-*>/agent/`
   - `models.yml`：從 `~/.omp/agent/models.yml` 複製（zai-coding provider + coding key）
   - `config.yml`：`modelRoles.default: zai-coding/glm-5.3`、`defaultThinkingLevel: max`
   - `mcp.json`：gbrain → `http://127.0.0.1:3131/mcp`（token 同 fleet）
2. 改 6 個 config.toml：
   `args = ["acp", "--profile", "<bot>", "--model", "zai-coding/glm-5.3", "--thinking", "max"]`
3. `launchctl kickstart -k` 逐一重啟，Discord 煙霧測試（每 bot 一次真實對話）
4. 驗證：`~/.omp/profiles/<bot>/agent/` 有各自 sessions/logs；`~/.omp/agent/` 不再被寫入（改名觀察或比對 mtime）
5. 回滾：config.toml 拿掉 `--profile` 兩字 + kickstart，即回今日現狀

## 已完成的相關修復（本次 session，2026-08-23/24）

- m4-free：config.toml `opencode-go/glm-5.3` → `zai-coding/glm-5.3`（兩處）；AGENTS.md 身分錯置（LCN-Pi-Design → M4-Free）已修、pi-design 殘留 skills/R2 路徑已清
- `~/.omp/agent/config.yml` modelRoles.default → `zai-coding/glm-5.3`（過渡期防呆，profile 化後各 bot 自帶）
- 全 fleet 無殘留 opencode-go 引用（掃 config.toml/plist/config.yml/models.yml）

## 風險與開放問題

- omp 版本升級後 profile 行為變更的回歸測試（低機率，18.0.1 已實測）
- janitor（com.openab.janitor）不是 omp，不在本方案範圍
- profile 化後 models.yml 有 6 份副本：coding key 輪替時要 6 處同步（寫個同步腳本一次解決）
