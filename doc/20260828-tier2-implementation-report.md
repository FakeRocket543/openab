# Tier 2 實作自驗報告 — chimera 合體 (A2) / elicitation / Discord 指令

> 日期:2026-08-28
> 方法:Fable Method + Ralph mode(subagent spawn → 逐一假說實測 → 部署 → pod e2e → self report)
> 前置:`doc/20260828-chimera-plan-mode-lifecycle.md`(原 Tier 0/1/2 分級設計)
> 結論先行:**Tier 2 最終以「零 code 改動」落地**——調查推翻原分級:/model 與 mode 切換指令上游已存在;elicitation 實測會卡死 omp ACP (拒用);真正有效的 A2 切換引擎是 `--plan-yolo-into`,已部署 vite 並 pod 級驗證。

---

## 1. 任務摘要

依指示「Tier 2 直接來」實作:2.1 ACP elicitation handler + 2.2 set_mode/Discord 指令。實際調查後發現原設計兩項假設錯誤,最終以配置變更完成合體目標。

## 2. 調查發現 (逐一實證,含推翻)

| # | 發現 | 證據 | 對設計的影響 |
|---|---|---|---|
| 1 | omp 公告 3 個 config 選項:`mode` (default/plan)、`model` (339 個)、`thinking` | 本地驅動 `omp acp` 抓 session/new 原始回應 (probe1) | mode/model 切換走**既有** set_config_option 管線,零新 RPC |
| 2 | Discord `/models` (category "model") 與 `/agents` (category "mode") **早已實作** | `discord.rs` L1531-1538;上游 commit `69118f3c`;部署版 pod binary grep 到字串 | **2.2 全部已存在**——同 session 重規劃 (`/agents`→Plan) 今天就能用 |
| 3 | 原生 `session/set_mode` RPC 不被 omp 支援 | probe1 實測回 `-32603 Unsupported ACP mode: undefined` | 不用;config option 路徑取代 |
| 4 | plan 核准在 ACP headless **自動通過**(無需 yolo 旗標、無 elicitation) | probe3:propose → `current_mode_update: default` → 直接實作 | 無人值守 OK;elicitation 非必要 |
| 5 | **宣告 `elicitation.form` 能力會讓 omp ACP prompt 整個停擺** | probe4/5:同流程,無宣告→完賽;有宣告→130 秒零輸出 (僅 4 個初始 update) | **2.1 拒用**——omp 18.0.8 缺陷;「原版行為」(自動核准) 即最佳 fallback,符合使用者預設 |
| 6 | `--plan` 旗標在 ACP **不換規劃模型**(兩種路徑皆否) | 中途切 plan mode:全 turn 仍 default;`--plan flash + plan-yolo`:全 turn flash (session jsonl per-turn model 實證) | 不使用 `--plan`;改用 `--model max` 起手 |
| 7 | **A2 真正的切換引擎是 `--plan-yolo-into`** | 本地: max→(規劃×3)→model_change flash→(實作×6);pod 18.0.5: 同序列 ×8 + pp.txt 建立 | 部署形態定案 |

## 3. 已部署變更

| 項目 | 內容 |
|---|---|
| OAB-K3D `values.yaml` (vite args) | `["acp","--model","zai/glm-5.3","--thinking","max","--plan-yolo","--plan-yolo-into","zai/glm-5.3-flash:high"]` |
| 部署 | `helm upgrade openab openab-0.10.0-beta.3.tgz -f values.yaml`;rollout 成功;pod `openab-lcn-chimera-vite-98866c75f-59m9g` |
| 確認不互相覆蓋 | `*​-omp` configmaps (heavy advisor: true) 不在 helm template 內 (grep 0 match),upgrade 後仍完好 (pod 驗證 L7) |
| OAB-K3D commit | `474ef51` |

## 4. 合體後的最終生命週期 (實測版,取代原設計文件 §2)

```
Discord @vite → session 起始 (--model max --plan-yolo)
 ▼ 強制 PLAN MODE
   glm-5.3:max 規劃 (唯讀探索 + local://計畫檔)
   xd://propose → 自動核准 (ACP headless 內建,非 yolo 專屬)
 ▼ --plan-yolo-into 自動切換
   glm-5.3-flash:high 實作 (可派 @heavy,max,自帶 advisor)
 ▼ 同 session 後續
   flash;隨時 /agents→Plan 重規劃 (max?否——見 caveat 1)、/models 換模型、/thinking 調深度
 ▼ session 結束 (TTL) → 下個 session 重走 max 規劃
```

## 5. Caveats

1. **`/agents`→Plan 重規劃用的是當前 model (flash)**——`--plan` 旗標不影響中途切換 (發現 #6)。要 max 重規劃:`/models` 先切 glm-5.3 → `/agents` Plan → 規劃 → 核准 → `/models` 切回 flash。三步,可用但不自動。
2. plan-yolo 為 session 起**一次性**——長 session 的第一個任務才有自動 max 規劃 (原設計已知限制,不變)。
3. `--plan-yolo` 使**每個新 session 的第一個任務**都走 max 規劃 (成本);小事密集的頻道建議另開 thread 或靠 /reset 控制 session 邊界。
4. plan-yolo 路徑未觀察到 `current_mode_update` 事件 (中途切換才有)——omp 觀測性小缺口,不影響功能。

## 6. Self Review (scout subagent 獨立調查)

- GatewayMap (唯讀 scout, 7 分鐘):完整描繪 /thinking→set_config_option 管線、request_permission 自動核准現況、config select menu 分頁基礎設施、classify_notification 的 mode 類缺口。本報告 §2 發現 #2/#5 的檔案行號證據即出自此調查。
- 實作前假說全部經 probe 實測;兩項設計假設 (elicitation 可用、--plan 有效) 被實測推翻並如實記錄。

## 7. Self E2E

| 驗證 | 結果 |
|---|---|
| probe1:omp config 選項公告 | ✅ mode/model/thinking |
| probe3:plan 自動核准 + mode 來回 | ✅ |
| probe4/5:elicitation 能力停擺重現 | ✅ (兩次) |
| probe (本地):A2 model_change 序列 | ✅ max→flash |
| **pod e2e (部署版 18.0.5)** | ✅ max 規劃×3 → flash 實作×8 → pp.txt 建立 |
| helm upgrade 後 heavy advisor 仍在 | ✅ L7 |
| 部署 args 進 config.toml | ✅ grep 實證 |

## 8. Self Improvements

1. omp 上游議題:`elicitation.form` 能力導致 ACP prompt 停擺 (18.0.8)——值得回報 upstream;修復後 2.1 可重新評估。
2. omp 觀測性:plan-yolo 路徑補發 `current_mode_update`;`--plan` 旗標語義與文件對不上 (ACP 下無效)。
3. openab `classify_notification` 可加 mode/config 類 update → Discord 狀態顯示 (Tier 1.2 仍有效,低優先)。
4. Discord `/plan` 別名指令 (= /agents 選 Plan) 可減少一步操作;`/models` 339 項分頁可用但搜尋體驗差,可加 fuzzy string option。
5. lcn-chimera (全 max pod) 與 vite (A2) 並存——觀察兩者成本曲線後決定是否退場一顆。

## 9. 最終判決

| 項目 | 判決 |
|---|---|
| 2.1 elicitation | **REJECTED (evidence-based)** — omp 缺陷,自動核准為正解 |
| 2.2 mode/model 指令 | **ALREADY EXISTS** — 上游 `69118f3c`,部署版已含 |
| A2 合體部署 | **VERIFIED** — 本地 + pod 雙重 e2e |
| Subagents/Spawn | **VERIFIED** — GatewayMap scout |
| Self E2E / Report / Improvements | **VERIFIED** — 本報告 |
| Ralph Mode | **VERIFIED** — 五輪假說 probe 直到定案,未中途中止 |

## 10. 版控

- OAB-K3D (sys101): `474ef51` (values.yaml A2 args)
- openab-fork: 本報告 + 修訂 lifecycle 文件 + NOTES (見 git log)

---
*Generated with omp main loop (GLM-5.3) / scout subagent / Fable + Ralph mode;evidence:本地 omp 18.0.8 probes ×7、pod omp 18.0.5 e2e、discord.rs/connection.rs/protocol.rs 行號實證。*
