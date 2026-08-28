# 中央技能供應、Hooks 採用與自動 Delegation — 需求與現況筆記

> 日期:2026-08-28
> 狀態:**筆記 / 待決策**(試點未開工)
> 適用對象:後續維護此系統的 subagents / 開發者
> 起因:對照官方 openab「水母 Agent 集群」概念圖 (ECS 版:自我發現 · 共享 AGENTS.md · 中央技能包 + ghpool),盤點本 fleet 的差距。結論先行:**k3d 形態 = 官方預期支援的 substrate,核心骨架同構;唯一未做到的是「中央動態供技能」與「bot 間自動 delegation」。**

---

## 一、需求 (本輪討論確認)

1. **中央動態供技能**:技能(與共用設定/憑證)集中於一處,更新一處、全 fleet 生效;bot 重建不依賴手動 patch。
2. **官方 hooks 機制採用評估**:`pre_seed` / `pre_boot` / `configUrl` 取代現行手工供應鏈。
3. **自動 delegation 評估**:現行「人類閘門 Discord @mention」是否/何時升級到官方 agent control plane (CP)。

## 二、現況 (2026-08-28 查證)

### 2.1 官方 hooks:**完全未使用**

證據:`openab-bots/*/config.toml`、`k8s/`、`charts/` 對 `pre_seed|pre_boot|pre_shutdown|post_boot|pre_session|post_session` 全數零 match。

實際使用的替代機制(全部手工/自建):

| 機制 | 內容 | 對應官方 ADR 位置 |
|---|---|---|
| Image 燒錄 + patch (C5 管線) | `cargo build → cp binary → docker build(openab-kiro:trixie-*) → k3d image import → kubectl rollout restart` | Approach A(官方建議淘汰:「every content change requires image rebuild」) |
| `openab-bots/sync-omp-profiles.sh` | 主樹 `~/.omp/agent` 的 models.yml (API key 輪替)、`.env`+`agent.db` auth 表 (devin re-auth)、`mcp.json` (gbrain token) → 6 profiles (m4-z/free/review/piswe/design/chimera),附 sha256 指紋驗證 | Layer 2 (s3 sync) 要解決的同一問題 |
| `kubectl cp` | omp binary → 各 pod PVC `~/.local/bin/omp` (atomic mv) | — |
| R2 (`k3d-lcntw-private`) | 僅 bot 間檔案交換 + presigned URL,**非** seed 來源 | — |

### 2.2 技能供應:per-bot 預掛

- 各 bot 自帶 `.devin/skills/`(如 m4-design 的 forgejo-api、diagram-design、video-use)+ AGENTS.md/CLAUDE.md/GEMINI.md + config.toml。
- 沒有任何中央技能庫;技能異動 = 逐 bot 手改。
- 官方標準已是 agentskills.io `SKILL.md`(hot 目錄/warm 本文 lazy load),native-agent 及各後端 (kiro `.kiro/skills/`、codex `.codex/skills/`) 皆同格式,**內容可直接搬**。

### 2.3 Delegation:三層現況

| 層 | 狀態 | 證據 |
|---|---|---|
| 官方協定碼 | **已實作**(upstream origin/main,非我方 fork) | `crates/openab-cp/`:完整 `cp/register / cp/delegate / cp/delegate_result / cp/cancel` wire protocol、AdmissionToken、深度/鏈/cycle/deadline 政策、namespace 隔離、飽和 fast-fail、測試齊全。ADR `agent-control-plane.md` 狀態仍標 **Proposed**(2026-08-06) |
| LLM 面 `spawn_agent` MCP 工具 | **未實作**——crates 全域 grep `spawn_agent` 零 match(僅 ADR 規格) | ADR §MCP facade 四工具 (spawn_agent / check_delegation / list_agents / cancel_delegation) |
| 我方 k3d 部署 | **未啟用 CP**;bot 間唯一通道 = Discord @mention(官方現行 multi-agent.md 模式) | bot config 無 `[cp]`;k8s/ 無 CP 元件 |

我方 bot 間委派規則(各 bot AGENTS.md「Bot-to-Bot Handoff」,刻意人類閘門):

- 禁止主動 @ 另一 bot。僅兩情況:①人類說「叫 @XXX 做」→ delegate 一次;②收到委派完成 → @ 回報一次,然後 STOP。
- 收到 bot 回覆一律忽略(anti-loop);收到任務立即執行不反問,缺資訊自行假設並註記。

**bot 內** subagent spawn 則已可用且常用:M4 自驗報告 (2026-08-20) 判決 `run_subagent` foreground 穩定、background `read_subagent` 回讀失敗(caveat);ralph mode 即以此做 subagent fan-out(`docs/reviews/*`:review → subagent fan-out → central E2E → fix → re-test → report)。

### 2.4 官方文件對應(查證路徑)

- `docs/adr/hooks.md`:三層供應(Layer 1 org tarball / Layer 2 s3 sync 技能+steering / Layer 3 static);`pre_seed` 從 S3 還原 HOME、`pre_shutdown` 打包回 S3;`pre_boot` last-mile `cp -rn /etc/openab/skills/ $HOME/.kiro/skills/`。
- `docs/adr/configurl-over-helm-rendering.md`:`configUrl`(S3/R2/HTTPS)一 URL 餵全 fleet,restart 即生效;`configToml` 僅 Helm 便利路徑。
- `docs/native-agent.md` + `docs/steering-design-guide.md`:SKILL.md 標準與 hot/warm/cold 分層。
- `.github/workflows/bundle-pre-seed-utils.yml`:官方 CI 自動 build pre-seed bundle (awscli + ghp) 供 `[hooks.pre_seed].sources`。
- `docs/refarch/kiro-with-defined-agents.md`:pre_seed 還原 `.kiro/agents`+skills 的完整範例。
- `[[skill:review]]` session 指令(runtime 技能啟動)= ADR 規劃中,未落地。

## 三、差距總表(概念圖 → 官方機制 → 我方)

| 圖中概念 | 官方機制 | 我方現況 | 差距 |
|---|---|---|---|
| 中央技能庫(動態拉) | pre_seed + S3 sync + `[[skill:…]]`(未落地) | per-bot 預掛 + 手同步 | **主要差距** |
| 共享 AGENTS.md/設定 | configUrl + Layer 1 tarball | sync-omp-profiles.sh(本機對本機) | 中:可用但手工 |
| 自我發現 | k8s Service DNS(已同構) | ✅ ghpool.openab.local 等 | 無 |
| ghpool PAT 池 | ✅ 官方元件 | ✅ 已部署 | 無 |
| bot 間協作 | Discord @mention(現行)→ CP delegation(ADR Proposed) | Discord @mention + 人類閘門 | 官方未出 LLM 工具前,維持現狀合理 |

## 四、候選行動(依序,未批准不開工)

1. **P1 試點:m4-free 改 pre_seed 架構**
   - R2 建 `s3://k3d-lcntw-private/seed/<bot>/base.tar.gz`(AGENTS.md + skills + config.toml),config.toml 加 `[hooks.pre_seed] sources` + `[hooks.pre_boot] inline` last-mile;驗證砍 pod 重建後首則訊息前即就緒。
   - 前提已滿足:pod 已持有 `$R2_ENDPOINT` 憑證。
   - 成功後 `sync-omp-profiles.sh` 的 models/auth/mcp 同步改推 R2 → 全 fleet restart 生效。
2. **P2:configUrl 遷移**——values/configmap 餵 config 改為 `-c s3://…`,消除 chart 維護。
3. **P3:CP delegation 觀察**——upstream 出現 `spawn_agent` MCP 工具或 ADR 轉 Accepted 後,再評估部署 `openab-cp`;在此之前維持人工閘門(與 anti-loop 規則一致)。

## 五、試點驗收標準(P1)

1. `kubectl delete pod` 後 pod 自動重建,無人工介入,首則 Discord 訊息可正常執行技能。
2. R2 更新技能包 → `rollout restart` → 新技能在 pod 內生效(md5sum 對照)。
3. `sync-omp-profiles.sh status` 指紋與 R2 內容一致。
