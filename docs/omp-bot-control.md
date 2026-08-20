# omp 系 openab bot 控制手冊

適用對象:以 `omp acp` 作為 ACP agent 後端的 openab bot(本 fleet:mecrivain、pi、pi-design、pi-z)。Devin 系(ecri-devin、lcn-review、lcn-swe、lcn-visual)走 `devin acp`,本文的 B/C 大部分不適用。

速覽卡片:[assets/omp-openab-control-card.svg](assets/omp-openab-control-card.svg)(2026-08-20 版)。

## 0. 架構與 session 模型

```
Discord ──WS── openab gateway(容器 PID1,Rust fork)
                   │  每則訊息 spawn,env 經 inherit_env 白名單過濾
                   ▼
               omp acp --model <model> [--thinking max]
                   │  HTTPS
                   ▼
               provider(zai / opencode-zen / …)
```

- **session 單位** = `discord:<channel_id>`(每 channel/thread 一個);`[pool] max_sessions = 10`、`session_ttl_hours = 24`。
- **spawn 生命週期**:agent 子進程隨訊息建立、turn 結束保留供 pool 重用;`/reset` 或 TTL 到期才銷毀。模型/模式切換存在 session 上,`/reset` 後回到 `config.toml` 預設。
- **env 邊界**:子進序只拿 `HOME/PATH/USER` + `inherit_env` 白名單 + `[agent].env`。gateway 自己(PID1)的 env 再多都不會漏給 agent —— 這是 2026-08-20 mecrivain 401 事故的根因(切 provider 沒同步白名單)。

## A. Discord slash 指令(使用者層,作用於該 channel 的 session)

| 指令 | 效果 | 適用情況 | 注意 |
|---|---|---|---|
| `/models` | 換本 session 的模型(選單分頁) | 臨時要更強/更便宜模型 | 不跨 `/reset`;catalog 含整個 omp 內建 + models.yml,很長 |
| `/agents` | 換 agent mode:**Default / Plan** | 見下方 Plan 模式 | fork `ae21db4` 才通(category `agent`→`mode`) |
| `/cancel` | 中止進行中的 turn | 回應卡住、方向錯了 | buffer 裡的後續訊息仍會處理 |
| `/cancel-all` | 中止 + 清空待處理 buffer | 連環誤發、洗版 | |
| `/reset` | 銷毀 session 重開 | context 污染、換主題 | 對話記憶與模型選擇一併消失 |
| `/remind` | 延遲提醒(mention) | 長任務回報 | |
| `/export-thread` | 匯出對話 | 存檔、跨工具分享 | |
| `/auth` `/usage` | 認證/用量 | — | 走 kiro 專屬通道(`_kiro.dev/commands/execute`),**omp bot 無效** |

機制:`/models`、`/agents` 都是讀 agent 在 `session/new` 公告的 `configOptions`,篩 category 後建選單,選擇經 `session/set_config_option` 下發。omp 17.3.x 只公告 `mode` 與 `model` 兩類 —— **沒有角色、沒有 thinking**,所以 Discord 端能做到的就是這兩個。

### Plan 模式(何時用)

進入後該 session:工具限縮為 `read/grep/glob/web_search`(agent 有宣告 `ast_grep` 才加)、先產出計畫(markdown)才准動工、禁止再派 subagent、prewalk 關閉。

- **適用**:寫作 bot 出大綱/故事聖經、研究調查、審視既有內容、任何「先想清楚再動手」的任務。
- **不適用**:需要直接執行(lint、上傳 R2、改檔)的收尾工作 —— 計畫完成後切回 Default 執行。

## B. 訊息內控制(寫在普通訊息裡,omp 端解析,per-turn)

### 魔術關鍵字

| 關鍵字 | 效果 | 適用情況 |
|---|---|---|
| `ultrathink` | 該 turn 注入多步推理要求;auto thinking 時拉到該模型最高 effort | 難題、要深思的單一提問;比全域 `--thinking max` 省 |
| `orchestrate` | 注入多代理編排契約:拆解、平行派工、逐段驗證 | 大範圍調查/重構/多檔修改 |
| `workflowz` | 注入 eval kernel 固定流程(`agent()/parallel()/pipeline()`) | 要確定性、可重複的批量任務 |

規則:小寫、獨立單字(`orchestrate,` 會中,`orchestrated` 不會);code block/行內 code 不觸發;只影響當前 turn。

### Directives(session 第一則訊息)

`[[ws:路徑或@alias]]` 指定 workspace、`[[title:標題]]` 指定標題 —— openab 層解析後從 prompt 剝除。詳見 [control-directives.md](control-directives.md)。

### 不適用:omp 的 `/...` 指令

那是 TUI client-side 指令;ACP headless 下到達 agent 的只是純文字(2026-08-20 實測)。在 Discord 打 `/model x` 不會換模型 —— 用 `/models` 選單。

## C. 部署端控制(營運層,不經 Discord)

### C1. 固定啟動參數 — values.yaml `configToml [agent]`

```toml
command = "/home/agent/.local/bin/omp"
args = ["acp", "--model", "zai/glm-5.3", "--thinking", "max"]
inherit_env = ["OPENCODE_API_KEY", "ZAI_API_KEY", "PI_SMOL_MODEL", …]
```

改這裡 = 改 bot 的出廠預設。**換 provider 時務必同步 `inherit_env`**(加新 key、可留舊的)。

### C2. 模型角色(成本控制的正確位置)

角色:`default` / `smol`(輕量:標題、prewalk 降級)/ `slow`(深思)/ `plan`(規劃)/ `advisor`(被動審查)。釘法二選一:

- **env(現行,fleet 推薦)**:deployment `env` 設 `PI_SMOL_MODEL=opencode-zen/mimo-v2.5`,並加進 `inherit_env`。優先權低於 CLI flag。
- **config.yml**:`/home/agent/.omp/agent/config.yml` 的 `modelRoles:`(可自訂角色名如 `review:`,agent frontmatter 用 `@review` 引用)。目前 PVC 內無此檔,全靠 env。

設計原則:**角色影響的是使用者看不到的內部開銷,釘在部署端而非開放 Discord 選**。

### C3. 自訂 subagent(人格 + 模型)

`~/.omp/agent/agents/*.md`(user 層,PVC 持久)或 `<cwd>/.omp/agents/*.md`(project 層,優先)。frontmatter 欄位:

| 欄位 | 作用 |
|---|---|
| `name` / `description` | 必填;description 供上層模型選派 |
| `model` | `@role` alias 或優先序列表;解析序:`task.agentModelOverrides` > 此欄 > parent 模型 |
| `thinking-level` | 該 agent 的 effort |
| `tools` | 限工具(CSV;自動補 `yield`) |
| `spawns` | `*` / 清單 / 空(禁再委派) |
| `output` | JSON schema(結構化回傳) |
| `prewalk` | 首次編輯後降級到便宜模型(`@smol` 或指定) |
| `advisor` | 掛被動審查者(`true` 或 `@smol:high`) |
| `read-summarize: false` | read 回原始內容(scout/librarian 內建如此) |

body = system prompt(人格)。內建 agent:`scout`、`designer`、`reviewer`、`security-reviewer`、`librarian`、`task`、`sonic`;`omp agents unpack` 可展開成檔案改寫,同名覆蓋。遞迴上限 `task.maxRecursionDepth = 2`。

### C4. provider / model 定義

ConfigMap `openab-pi-z-models` → 掛 `/home/agent/.omp/agent/models.yml`(現定義 zai/glm-5.3,anthropic 相容端點,`apiKey: ZAI_API_KEY` 環境參照)。加 provider 就改這個 CM。

### C5. 版本與部署管線

| 元件 | 位置 | 更新方式 |
|---|---|---|
| omp | PVC `~/.local/bin/omp` | host 官方 binary → `kubectl cp` 進各 pod(atomic mv;下一則訊息生效) |
| openab | image `openab-kiro:trixie-*` | fork 改 → `cargo build --release -p openab` → `cp` 到 `OAB-K3D/image/openab` → `bash OAB-K3D/image/build.sh`(docker build + k3d import)→ `kubectl rollout restart` |
| values/chart | `~/OAB-K3D/values.yaml` | `helm package` + `helm upgrade`(見 `/tmp/chart-deploy.sh`) |

## D. 情境 → 工具對照

| 情境 | 用什麼 |
|---|---|
| 這題想換更強/更省的模型 | `/models` |
| 先規劃再動工(大綱、研究) | `/agents` → Plan,完稿切回 Default |
| 單一問題要深思 | 訊息加 `ultrathink` |
| 大範圍平行調查/重構 | `orchestrate`;要可重複流程用 `workflowz` |
| 指定工作目錄 | 首訊息 `[[ws:@alias]]` |
| bot 回應卡死 | `/cancel`;誤發洗版 `/cancel-all` |
| 對話壞掉/換專案 | `/reset` |
| 常駐成本壓低 | 部署端 `PI_SMOL_MODEL` 等角色釘選;`/models` 臨時選便宜模型 |
| 固定人格的分工後端 | `~/.omp/agent/agents/*.md` + `@role` |
| 換 provider | models.yml + `inherit_env` + args 三處同步 |

## E. 故障排查(本 fleet 實例)

### 401 "token expired or incorrect"(2026-08-20)

症狀:mecrivain 每 turn 失敗,重啟無效。**該訊息是誤導** —— z.ai 對「無效/缺失憑證」的統一文案。鑑別法:

1. key 是否有效:pod 內 `curl` provider 端點帶 PID1 env 的 key → 200 即有效。
2. 子進程有沒有拿到 key:模擬白名單環境跑 `omp -p --model …`:
   ```sh
   env -i HOME=/home/agent USER=agent TERM=xterm PATH=… ZAI_API_KEY="$ZK" \
     ~/.local/bin/omp -p --model zai/glm-5.3 "say OK"
   ```
   有 key → OK、無 key → 重現 401,即 `inherit_env` 漏項。
3. omp log 證據:`~/.omp/logs/omp.*.log` 的 `agent turn ended with provider error`。

### 其他坑

- **provider 切換三件套**:models.yml、args、`inherit_env` 必須同步;漏一個就是 401 或 fallback 錯模型。
- **free tier 卡住**:`opencode-zen/mimo-v2.5-free` 會卡,角色釘選一律用非 free(`mimo-v2.5`)。
- **`/models` 洗版**:catalog 全列,靠分頁;若要收斂可在 omp 端縮 catalog,目前未做。
- **PVC 重建會掉**:omp binary、agents/*.md、config.yml 都在 PVC;models.yml 走 CM 不會掉。持久化關鍵設定應進 CM 或 values。

## 附:現況快照(2026-08-20)

- omp 17.3.8(4 pod),openab fork `ae21db4`(image `openab-kiro:trixie-*`)
- 修復:mecrivain `inherit_env` + `ZAI_API_KEY`;`/agents` category → `mode`;`PI_SMOL_MODEL=opencode-zen/mimo-v2.5` ×4
- 公開卡片:`https://pub-4421302ef6b240e1a9b6d88a9731aa19.r2.dev/main/omp-openab-control-20260820.svg`
