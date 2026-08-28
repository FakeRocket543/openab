# Chimera 合體設計:plan mode 生命週期與 openab-fork 配合修改

> 日期:2026-08-28
> 狀態:**設計記錄 / 待批准實施**
> 前置:`doc/20260828-omp-vision-advisor-fixes.md`(heavy advisor 已部署)、lcn-chimera-vite 合體可行性報告 (R2 `chimera-merge-report-20260828.md`,A1/A2 建議)
> 結論先行:**形態 2 (plan-yolo) 上線零 code——只改 configmap args;互動規劃與 Discord 換檔才需要動 openab-fork (分兩級)**

---

## 1. 合體後的完整編制 (目標形態)

```toml
# openab configmap (/etc/openab config.toml) [agent] args
args = ["acp", "--model", "zai/glm-5.3-flash:high",
        "--plan", "zai/glm-5.3:max",
        "--plan-yolo", "--plan-yolo-into", "zai/glm-5.3-flash:high"]
```

四層:Flash 主迴圈 (95%) + `--plan` max 規劃 (plan-yolo 自動核准) + `@heavy` max 深度執行 (按需,自帶 advisor) + Devin advisor 全程旁觀。

## 2. 模式生命週期

```
Discord @bot → openab session (channel/thread 為單位,TTL 回收)
 ▼ session 啟動 (--plan-yolo 強制)
┌ PLAN MODE (唯讀) ────────────────────────────┐
│ model = plan role (glm-5.3:max)              │
│ 可探索不可寫;advisor 旁看 (concern→卡片)    │
│ agent 寫 xd://propose → PlanYolo 自動核准    │
└──────────────┬───────────────────────────────┘
               ▼ 切換 --plan-yolo-into
┌ IMPLEMENTATION (一般模式) ───────────────────┐
│ model = flash:high;按 plan 實作             │
│ 深度需求 → 派 @heavy (max, advisor:true)     │
│ advisor 全程 (concern 轉向 / blocker 再開)   │
└──────────────┬───────────────────────────────┘
               ▼ 同 session 後續訊息
       一般 flash 回合 (plan-yolo 已消耗,不重規劃)
       heavy 隨時可派;advisor 持續
               ▼ session 結束 → 下個 session 重新規劃
```

關鍵事實:
1. **形態 1 (`--plan` 不帶 yolo) 在 headless bot 是死棋**——plan mode 進入路徑僅 TUI 快捷鍵 / session 起始旗標 / openab 主動 ACP 下指令 (未實作);agent 不能自切
2. **plan-yolo 為 session 起一次性**——粒度 = 每個 channel/thread 的第一個任務;長 session 後續任務不重規劃 (解法見 Tier 2)
3. **ACP plan 核准走 elicitation,openab 未實作 elicitation handler**——故無人值守必須 yolo 自動核准 (實證:`crates/openab-acp/src/connection.rs` 僅處理 `session/request_permission`)

## 3. openab-fork 配合修改 (分級)

### Tier 0 — 零 code,即可上線 (形態 2)

| # | 項目 | 層級 |
|---|---|---|
| 0.1 | args 加 `--plan`/`--plan-yolo`/`--plan-yolo-into` | OAB-K3D configmap (非 code) |
| 0.2 | 驗證 omp 發出的 session/update (mode 類) 被 openab 忽略不炸 | 實測 (上線前 5 分鐘驗證,見 §5) |
| 0.3 | `prompt_hard_timeout_secs` 已 3h (0e3e1c0),plan+實作同 prompt 流程足夠 | 已完成 |

### Tier 1 — 小改 (體驗補強,各自獨立可做)

| # | 項目 | 改動點 | 依據 |
|---|---|---|---|
| 1.1 | Discord `/model` 指令 → ACP `set_model` | `connection.rs` 仿既有 `session/set_config_option` 管線 (L849) 加一個 RPC + gateway 指令映射 | merge 報告 B;fork 已有 /thinking 前例 (omp-acp-compat.md) |
| 1.2 | session/update mode 類 → Discord 狀態顯示 (目前模型/plan 中) | gateway adapter | 同 shell-pushmode 狀態燈模式 |

### Tier 2 — 中改 (解鎖互動規劃)

| # | 項目 | 改動點 | 效果 |
|---|---|---|---|
| 2.1 | ACP elicitation handler | `connection.rs` 加 `session/request` (elicitation) 分支,仿 request_permission (L370) 的 pending-response 模式 → Discord 按鈕/回覆核准 | plan 核准不再需要 yolo → **形態 1 復活** (互動式改計畫) |
| 2.2 | 發送 `session/set_mode` + omp 端支援確認 | connection.rs 新 RPC + omp ACP server 驗證 | Discord `/plan` 指令 → **同 session 手動重進 plan mode** (解決一次性限制) |

## 4. 使用策略 (合體後)

| 情境 | 做法 |
|---|---|
| 新任務要 max 規劃 | 開新 thread @bot (plan-yolo 自動跑) |
| 同 thread 小事 | 直接問,flash 處理 |
| 同 thread 深活 | 「派 heavy 做 X」→ flash 派 @max |
| 互動式改計畫 | 需 Tier 2.1/2.2 落地後 |

## 5. 上線前驗證清單 (Tier 0 gate)

1. vite pod 手動:`~/.local/bin/omp -p --plan zai/glm-5.3:max --plan-yolo "規劃:〈小任務〉"` → transcript 應見 `model_change`:規劃回合 glm-5.3 → resolve 後 flash
2. openab 側:跑一則 Discord 訊息觸發 plan 流程,確認 openab log 無未處理 frame 錯誤
3. `omp stats` 對比切換前後 token 成本 (規劃回合 max 費用量測)

## 6. 決策點 (待使用者定奪)

- [ ] Tier 0 直接上 vite?(建議:是,驗證清單過了就上)
- [ ] Tier 1.1 `/model` 指令要不要做?(建議:做,改動小、立刻補上 B 選項)
- [ ] Tier 2 何時排?(建議:等 Tier 0 實測數據後再決定)

---
*設計討論記錄:omp main loop (GLM-5.3) + advisor;證據:omp cli-reference/prewalk/resolve-tool-runtime/advisor-watchdog 文件、openab-acp connection.rs 實測 grep。*

---

## 7. 實作結果修正 (2026-08-28 同日晚,詳見 doc/20260828-tier2-implementation-report.md)

原 §3 分級經實測大幅修正:
- **2.2 已存在**:`/models`、`/agents` 指令上游 `69118f3c` 即有,部署版 binary 已含;mode 切換走 omp 公告的 `mode` config 選項 (default/plan),零新 RPC。
- **2.1 拒用**:宣告 `elicitation.form` 能力會使 omp ACP prompt 停擺 (probe ×2 重現);ACP headless 的 plan 核准本就自動通過——即最佳 fallback。
- **`--plan` 旗標在 ACP 無效** (兩路徑實證);A2 切換引擎 = `--plan-yolo-into`。
- **A2 已部署 vite 並 pod e2e 驗證**:max 規劃 ×3 → 自動切 flash 實作 ×8。OAB-K3D `474ef51`。
