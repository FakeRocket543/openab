# OMP Fleet 配置修正自驗報告 — vision role / heavy advisor

> 日期:2026-08-28
> 方法:Fable Method + Ralph mode(subagent spawn → self review → fix → self e2e → re-test → self report)
> 範圍:`~/.omp/agent/` 主樹 + bot profiles。openab-fork repo 僅收錄本報告(omp 配置不在版控)。

## 1. 任務摘要

三項核可修正:

1. **金鑰檔權限** (`~/.omp/agent/.env` 及各 profile `.env`) → 檢查後**全數已是 `-rw-------` (600)**,免改。
2. **vision role 改影像支援模型**:glm-5.3 系不收圖片輸入(實測 `inspect_image` 報錯),改 `opencode-go/muse-spark-1.2-contributor`。
3. **heavy agent 掛 advisor**:m4-chimera 的 `agents/heavy.md` 加 `advisor: true`,派重腦時由 Devin SWE-1.7 advisor 審查。

## 2. 完成標準

YAML 全部可解析、無舊值殘留、模型可解析(含白名單)、headless 冒煙 `omp -p` 兩棵樹皆 OK、subagent 獨立審查通過。

## 3. 變更總覽

| 檔案 | 變更 |
|---|---|
| `~/.omp/agent/config.yml` | `vision: zai-coding/glm-5.3-flash` → `opencode-go/muse-spark-1.2-contributor` |
| `~/.omp/profiles/m4-chimera/agent/config.yml` | 同上 |
| `~/.omp/profiles/m4-free/agent/config.yml` | 同上 (原帶 `:max` 後綴,改為 bare) |
| `~/.omp/profiles/m4-free/agent/config.yml` (第二處) | `enabledModels` 白名單補 `opencode-go/muse-spark-1.2-contributor`(審查發現的回歸,見 §5) |
| `~/.omp/profiles/m4-chimera/agent/agents/heavy.md` | frontmatter 加 `advisor: true`(六鍵齊:name/description/model:"@slow"/thinking-level:blocking/advisor) |

## 4. Self E2E 結果

- YAML:`python3 yaml.safe_load` 4/4 OK。
- 冒煙:`omp -p "reply with exactly: OK"`(主樹)→ **OK**;`omp --profile m4-free -p …` → **OK**(完整載入含白名單)。
- 殘留掃描:三棵樹無任何 `vision: zai-coding/glm-5.3-flash` 殘留;`retry.fallbackChains.vision → @vision2` 引用完整(main/chimera 的 vision2 角色存在)。
- `.env` 權限:6 個存在檔全 600。

## 5. Self Review(subagent 獨立審查)

以 task 工具 spawn 唯讀 scout 審查 5 項:**4 PASS、1 FAIL**。

- FAIL 項(本次導入的回歸):m4-free 的 `enabledModels` 為非空白名單(僅 5 模型),未含 muse-spark → vision 角色指向被停用模型,且該 profile 無 vision2、models.yml 亦無 muse-spark,替代解析條款不成立。
- **修復**:白名單補第 6 項 → 重驗 YAML OK + `--profile m4-free` 冒煙 OK。
- 其餘確認:muse-spark 經 opencode-go discovery provider 可解析(`disabledProviders` 僅 bedrock 系);`advisor:` 為 agent frontmatter 合法鍵(task-agent-discovery 文件明載);`@slow` 別名指向 chimera 的 `zai-coding/glm-5.3:max` 可解析。

## 6. Self Improvements(建議,未執行)

1. `m4-design`、`m4-z` 等其餘 profile **未定義 vision role**(vision 會落到不支援影像的 default)——若這些 bot 要做影像工作需比照補;現況不影響,留待決策。
2. vision 與 vision2 同模型後,`fallbackChains.vision → @vision2` 語意冗餘(無害),未來清理。
3. `models.yml` 僅顯式定義 zai-coding;muse-spark 靠 discovery provider 解析——建議顯式定義降低環境依賴。
4. `sync-omp-profiles.sh` 僅同步 models/auth/mcp,不含 `config.yml` 的 modelRoles——本輪 3 處 vision 採手改,建議未來把 modelRoles 納入同步或改走 pre_seed 中央化(doc/20260828-central-skills-hooks-delegation.md P1)。

## 7. 最終判決

| 項目 | 判決 | 說明 |
|---|---|---|
| vision role ×3 | **VERIFIED** | 含 m4-free 白名單回歸修復 |
| heavy advisor | **VERIFIED** | frontmatter 六鍵、@slow 可解析、advisor role=devin/swe-1.7:max + enabled |
| 金鑰權限 | **VERIFIED (原已達標)** | 全 600,免改 |
| Subagent/Spawn | **VERIFIED** | scout 唯讀審查實跑,抓到真回歸 |
| Self E2E | **VERIFIED** | YAML 4/4 + 冒煙 2/2 |
| Self Improvements | **VERIFIED** | §6 四項可執行建議 |
| Ralph Mode | **VERIFIED** | 審查 FAIL → 修復 → 重測通過,未中途中止 |

## 8. Ralph 結論

所有子任務完成或記錄 caveat;唯一 FAIL(m4-free 白名單)已在同一循環內修復並重驗。本報告即驗證證據。

---
*Generated with omp main loop (GLM-5.3) / scout subagent review / Fable + Ralph mode.*
