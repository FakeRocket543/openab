# 本機 OpenAB bots 與 sys101 k3d 環境隔離方案（方案 A：omp --profile）

日期：2026-08-24
狀態：**已實施完成（同日）**
背景事故：2026-08-24，停用 opencode-go key 後全本機 bots 連帶 401，根因為共享 `~/.omp/agent/`。

## 問題

本機 6 bots（LaunchAgent 啟動）中，4 隻 omp bot 全部讀同一份 omp 狀態：

- `~/.omp/agent/config.yml` — modelRoles.default（曾指 opencode-go → 單點炸全隊）
- `~/.omp/agent/models.yml` — provider zai-coding + key
- `~/.omp/agent/mcp.json` — gbrain MCP 接線
- `~/.omp/agent/{agent.db,sessions/,logs/}` — 狀態與歷史

m4-design、m4-devin-swe 用 devin CLI，不讀 `~/.omp/`，天生隔離，不在此方案範圍。

## 決策與實施

採 **方案 A：omp `--profile`**（B 隔離 HOME、C 本機容器化均否決）。

已實施（2026-08-24）：

1. 為 4 隻 omp bot 各建 profile `~/.omp/profiles/<m4-*>/agent/`：
   - `models.yml`（zai-coding provider + coding key，自主樹複製）
   - `config.yml`（`modelRoles.default: zai-coding/glm-5.3`、`defaultThinkingLevel: max`）
   - `mcp.json`（gbrain → `http://127.0.0.1:3131/mcp`，token 同 fleet）
   - `.env` + `agent.db`（devin provider auth：`auth_credentials` 5 筆，m4-review/m4-piswe 跑 `devin/swe-1-7` 需要）
2. 4 個 config.toml args 改為
   `["acp", "--profile", "<bot>", "--model", <原模型>, "--thinking", "max"]`
   （備份 `.bak-20260824`）
3. 6 bots 全部 `launchctl kickstart -k` 重啟，全數正常（exit=0）
4. 驗證：
   - `omp --profile m4-z --model zai-coding/glm-5.3` → OK-Z
   - `omp --profile m4-review --model devin/swe-1-7` → OK-REVIEW2（auth 複製後）
   - 各 profile 樹有自己的 sessions/logs；主樹 `~/.omp/agent/` 隔離後零新寫入
   - profile 內 log 僅良性警告（ollama/lmstudio discovery、fetch/deepwiki stdio MCP 本來就 flaky）

## 維運注意

- **coding key 輪替**：要同步 `~/.omp/profiles/{m4-z,m4-free,m4-review,m4-piswe}/agent/models.yml` 4 份 + 主樹 1 份 = 5 處（用同步腳本）
- **devin auth 過期**：重登後要重copy `agent.db`（或只搬 `auth_credentials` 表）到 4 個 profile
- bare model name（`glm-5.3` 不帶前綴）仍會 fuzzy-match 到任何 provider 目錄 → 一律用全名 `zai-coding/glm-5.3`
- 回滾：config.toml 拿掉 `"--profile", "<bot>"` 兩元素 + kickstart，即回 2026-08-24 前狀態

## 相關修復（同日）

- m4-free：`opencode-go/glm-5.3` → `zai-coding/glm-5.3`（config.toml 兩處）；AGENTS.md 身分錯置（LCN-Pi-Design → M4-Free）修正、pi-design 殘留 skills/R2 路徑清除
- `~/.omp/agent/config.yml` modelRoles.default → `zai-coding/glm-5.3`（主樹防呆，保留）
- 全 fleet 無殘留 opencode-go 引用
