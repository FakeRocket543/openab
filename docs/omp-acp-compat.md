# omp × openab ACP 相容性報告

- 日期:2026-08-26
- 範圍:本機 `openab` 0.10.0(fork,`/Users/fl/Python/oab`)× 本機 `omp` 18.0.6(`~/.bun/bin/omp`)以 ACP 對接時,雙方「特殊指令/特殊模式」的可用性
- 方法:`omp acp` live JSON-RPC 探測(附錄 A)+ 兩邊原始碼/文件比對;fleet 現況對照 [omp-bot-control.md](omp-bot-control.md)(2026-08-20 版,基於 omp 17.3.8)

## TL;DR

1. **大部分相容**:openab 的 Discord 指令全走協議方法;omp 有 **116 個** text-capable slash 指令可在 ACP 下真執行;魔術關鍵字(`ultrathink`/`orchestrate`/`workflowz`)純文字解析,天然相容。
2. **兩邊都沒有叫「loop mode」的功能**。最接近的 `/vibe` 明確 TUI-only;可用的替代是 `/autoresearch`(ACP 可觸發)、advisor watchdog(受 `deferAgentInitiatedTurns` 閘門)、以及 openab 層的 cronjob / ambient / PR auto-fix。
3. **「指定角色」不能也不該從聊天端切換**:角色的 modelRoles 不在 ACP 公告裡;正解維持部署端釘選 + openab multi-agent(一 bot 一人格)。
4. **現成落差一個**:18.x 新增的 `thinking` configOption 類別,openab 的 category 白名單沒放行 → 沒有 Discord 入口。**已在本 fork 實作修復**(commit `6b6ca7e8`,見 §6)。

## 1. 架構

```
Discord ──WS── openab gateway ──stdio ACP── omp acp(每 discord:<channel_id> 一個子進程)
                 │
                 ├─ /models  → 讀 session/new 的 configOptions(category=model)建 Select Menu
                 ├─ /agents  → 同上(category=mode,fork ae21db4 起同時接受舊 category=agent)
                 │             選擇經 session/set_config_option 下發
                 ├─ /cancel  → session/cancel notification
                 └─ /reset   → 銷毀 session pool entry(模型/mode 選擇一併歸零)
```

- session 單位 = `discord:<channel_id>`;spawn 隨首則訊息,turn 後保留供 pool 重用(`/reset` 或 TTL 才銷毀)。
- env 邊界:子進程只拿 `HOME/PATH/USER` + `inherit_env` 白名單 + `[agent].env`(mecrivain 401 事故的教訓)。

## 2. 實測:omp 18.0.6 ACP 公告能力

對 `omp acp` 送 `initialize` → `session/new`,回應如下:

| 項目 | 內容 |
|---|---|
| `modes.availableModes` | `default`("Standard ACP headless mode")、`plan`(僅當設定 `plan.enabled` 開啟才公告);切換走標準 `session/set_mode` |
| `configOptions[mode]` | category=`mode`,Default/Plan |
| `configOptions[model]` | category=`model`,整個 model catalog(很長) |
| `configOptions[thinking]` | category=`thought_level`,Off/Auto/low/high/max —— **18.x 新增**,fleet 卡片時代(17.3.x)不存在 |
| available commands | **116 個**(見 §3.1 節錄) |

原始碼對應(bundle 反查):modes 定義僅 default+plan;`#ue()` 對未知 mode 擲 `Unsupported ACP mode`;plan mode 切換時掛 `xd://propose` proposal handler。

## 3. 相容矩陣

### 3.1 ✅ 可以在 ACP 中執行

| 類別 | 例 | 機制 |
|---|---|---|
| openab Discord 指令 | `/models` `/agents` `/cancel` `/reset` `/remind` `/export-thread` `/auth` | 協議方法(`set_config_option`/`cancel`),與 agent 種類無關 |
| omp text-capable 內建指令 | `/compact` `/handoff` `/shake` `/usage` `/context` `/stats` `/tools` `/advisor` `/retry` `/fresh` `/memory` `/rename` `/add-dir` `/mcp` `/ssh` `/jobs` `/todo` `/autoresearch` `/green` `/review` `/security` `/force` `/prewalk` `/fast` …共 116 個 | 統一 built-in registry 在 `AgentSession.prompt()` 前分派;TUI-only 的不廣播也不處理(slash-command-internals.md §5、§ACP/RPC availability) |
| 檔案式指令 | SuperClaude `/sc:*`、`/deploy` 等(`~/.claude/commands` 等發現路徑) | 展開成 prompt 送入,正常運作 |
| 魔術關鍵字 | `ultrathink` `orchestrate` `workflowz` | omp 解析訊息文字本身,per-turn 生效,與 transport 無關 |
| openab directives | `[[ws:@alias]]` `[[title:…]]` | openab 層剝除後才進 ACP |
| subagents / 人格派工 | task tool 內 spawn scout/designer/reviewer/自訂 agents | in-process,ACP 只是入口,不影響 |

注意:`@bot /model xxx` **不會**換模型 —— `/model` 在 ACP 只剩「顯示目前模型」的 text handle,切換一律走 `/models` 選單。

### 3.2 ❌ 不相容(TUI-only)

| 指令 | 原因 |
|---|---|
| `/pause` | process-global TUI gate |
| 模型 selector UI(`/model` 的切換面) | 互動式選擇器只在 TUI |
| `/vibe`(director + persistent workers 迴圈) | 文件明載 "interactive-TUI command";進出模式與 worker 生命週期依賴 TUI |

這些連 `available_commands_update` 都不公告;從 Discord 打字進去只是純文字掉進 prompt。

## 4. 特殊模式盤點

### 4.1 「loop mode」——兩邊都沒有這名字的功能

| 近似物 | 所屬 | ACP 相容? | 備註 |
|---|---|---|---|
| `/vibe` 持久 worker 迴圈 | omp | ❌ | 明確 TUI-only |
| `/autoresearch <goal>` 自主研究迴圈 | omp | ⚠️ | 有 text handle → 聊天可觸發;per-project DB/runs 的長時任務,headless 完整循環未實測 |
| advisor watchdog 自動續跑 | omp | ✅* | 受 `deferAgentInitiatedTurns` 閘門:bridge 未允許 agent-initiated turn 時,advice 降級為卡片不重啟 run(advisor-watchdog.md) |
| cronjob 排程任務 | openab | ✅ | broker 層排程,fire-and-forget,不佔 chat turn |
| ambient mode | openab | ✅ | 連續聆聽 buffer,`max_bot_turns` cap 防發散 |
| PR auto-fix label 迴圈 | openab | ✅ | GitHub webhook 驅動,label 自移除防再入 |
| bot-to-bot 協作迴圈 | openab | ✅ | `allow_bot_messages="mentions"`(自然斷路器)+ 10 連續 bot turn 硬上限 |

**結論**:想要「循環式自主工作」,正確位置是 broker 層(cronjob/workflowz/orchestrate 契約),不是在 ACP 之上再造 agent-side loop —— 那會跟 openab 刻意設計的 no-mid-turn-interrupt、turn-boundary batching 對撞。

### 4.2 「指定角色的 mode」

omp 的角色體系:

- **model roles**:`default/smol/slow/plan/advisor`(+ `config.yml modelRoles` 自訂如 `review:`),供 prewalk/handoff/task agent 以 `@role` 引用
- **agent 人格**:`~/.omp/agent/agents/*.md`(user)/ `<cwd>/.omp/agents/*.md`(project),frontmatter 定義 model/thinking/tools/spawns/output,body = system prompt

兩者**都不在** ACP `configOptions` 公告裡(只有 mode/model/thinking 三類)。所以:

- Discord 端**無法**即時切角色 —— 這是 omp 端就不公告,不是 openab 缺功能;
- fleet 文件 C2 的設計原則成立:「角色影響的是使用者看不到的內部開銷,釘在部署端」(`PI_SMOL_MODEL` env / `args --thinking`);
- 「固定人格的分工後端」正解 = openab multi-agent:每個 `[agents.<name>]` 一個 Deployment + bot token(mecrivain/pi/pi-design/pi-z 即此模式)。

## 5. 已知落差與機會

1. **`thinking` 類別無 Discord 入口**(已修):fork commit `6b6ca7e8` 新增 `/thinking` 指令(category=`thought_level`),omp 端以 `set_config_option(configId="thinking")` 實測成功(§6 驗證記錄)。
2. **`/models` 洗版**:omp 回傳整個 catalog,Discord Select Menu 上限 25 筆/頁靠分頁撐;收斂要在 omp 端縮 catalog(fleet 已知,未處理)。
3. **`/autoresearch` 在 headless 的完整行為未實測**:有公告、有 text handle,但 runs/ASI 迴圈是否依賴 TUI 狀態待驗證。
4. **ACP 無 per-session approval-policy 欄位**:要 yolo 得另起 `omp acp --yolo` 或帶 `--config` overlay(approval-mode.md;17.x 起 flags 存在但 `--help` 未列)。

## 6. fork 修改評估

現況:fork 已領先 origin 53 commits,且有前例 —— `42e01b77 fix(discord): accept both mode and agent categories for slash /agents` 就是同一種改動;部署管線完整(C5:cargo build → image → k3d import → rollout restart)。

### 已實作:`/thinking` 選單(fork commit `6b6ca7e8`,2026-08-26)

改動(`crates/openab-core/src/discord.rs`,仿 `/models`):

1. command registration:新增 `/thinking`;
2. dispatch:`handle_config_command(&ctx, &cmd, "thought_level", "thinking level")`;
3. pagination whitelist 加入 `"thought_level"`;
4. `docs/slash-commands.md` 指令表補列。

驗證:`cargo check -p openab-core` 通過;`cargo test -p openab-core discord` 全數通過(含新增 category-matching 測試:`thought_level` 命中、`mode`↔`agent` fallback、缺類別 → None);omp 18.0.6 實測 `session/set_config_option(configId="thinking", value="low")` → 成功回傳且收到 `config_option_update`(thinking → low)。

部署提醒:此改動在 gateway binary,需走 C5 管線(cargo build → image → k3d import → rollout restart)才會生效;Discord 端 guild 指令註冊在 bot ready 時自動更新。

審查補記(2026-08-26 self-review):

- `thought_level` 是 ACP 官方 schema 的一級 category(`acp_schema.rs` 枚舉:`mode`/`model`/`model_config`/`thought_level`/`other`)—— 本改動對齊規格,非 omp 專屬 hack;`model_config` 是下一個候補,等有 agent 公告再放行。
- `connection.rs` 的既有 fallback:agent 拒絕 `set_config_option` 時,openab 會把 `/thinking <value>` 當 prompt 文字送出並本地改 `current_value`。對 omp 不觸發(實測協議路徑成功);對假想的「公告 thought_level 但不支援該方法」的 agent,該文字會落入 prompt —— 與 `/model` fallback 同型,不改。
- kiro 等僅 `models`/`modes` fallback 的 backend:configOptions 無 `thought_level` → `/thinking` 顯示既有的「No thinking level options available」ephemeral 訊息,與 `/agents` 對不支援 agent 的行為一致。

### 不建議做

| 提案 | 理由 |
|---|---|
| 自幹 agent-side「loop mode」 | 與 openab 架構前提(no mid-turn interrupt、turn-boundary batching、bot-turn caps)正面衝突;需求已被 cronjob + workflowz/orchestrate 覆蓋 |
| 把 modelRoles/角色做成 Discord 選單 | omp 根本沒公告 → 得在 openab 端 re-spawn session 換 args,破壞 pool/session 模型;且違背自家「角色釘部署端」原則。多 bot 已是正解 |
| 暴露更多 omp 內部狀態 | fork 落差越大,追 upstream(ADR 迭代快)越痛 |

### 維護原則

- 維持「小而加法」的 patch 集,每個 fix 一個 commit(現行做法正確);
- **fork-only**:不推回上游 —— 上游近期動力在其他主題、未必收件;改動直接落在自家 fork(`ssh://git@git.lcn.tw:2222/felix/openab-fork.git`,本地 remote 名 `forgejo`),分歧成本自行吸收即可;
- omp 端升級後重跑附錄 A 探測,比對 `configOptions` 類別變化再決定要不要加選單。

## 附錄 A:ACP 探測重現

```python
import json, subprocess, threading, queue
p = subprocess.Popen(["omp", "acp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, cwd="/tmp")
q = queue.Queue()
threading.Thread(target=lambda: [q.put(json.loads(l)) for l in p.stdout if l.strip()], daemon=True).start()
def send(o): p.stdin.write((json.dumps(o)+"\n").encode()); p.stdin.flush()
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}})
send({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}})
import time; end=time.time()+60
while time.time()<end:
    try: m=q.get(timeout=1)
    except queue.Empty: continue
    r=m.get("result")
    if m.get("id")==2 and r:
        print("MODES:", json.dumps(r.get("modes")))
        print("CONFIG:", json.dumps([{k:(v if k!="options" else [o.get("name") for o in v][:8])
              for k,v in o.items()} for o in r.get("configOptions",[])]))
    u=m.get("params",{}).get("update",{})
    if u.get("sessionUpdate")=="available_commands_update":
        print("COMMANDS:", len(u["availableCommands"])); break
p.terminate()
```

## 附錄 B:證據索引

| 主張 | 出處 |
|---|---|
| modes 僅 default/plan;plan 需 `plan.enabled` | bundle:`#J(e)`(`GUo="default", nOs="plan"`),live probe MODES 輸出 |
| configOptions 三類(mode/model/thinking) | bundle:`#U(e)` builder(`oOs="mode", rOs="model", iOs="thinking"`),probe CONFIG 輸出 |
| 116 個 ACP 指令 | probe `available_commands_update` |
| built-in registry 在 prompt 前分派;TUI-only 不進 ACP | `omp://slash-command-internals.md` §5、§ACP/RPC availability |
| `/vibe` TUI-only | `omp://vibe-mode.md` |
| advisor 於 ACP 受 `deferAgentInitiatedTurns` | `omp://advisor-watchdog.md`(§132 附近) |
| ACP approval 無 per-session 欄位;`--yolo/--approval-mode/--config` 用法 | `omp://approval-mode.md` §ACP sessions |
| openab category filter 與白名單 | `crates/openab-core/src/discord.rs`:1572-1582、1670-1692、2612;commit `42e01b77` |
| Discord 指令集與 @mention 轉發 | `/Users/fl/Python/oab/docs/slash-commands.md` |
| fleet 現況、env 白名單、角色釘選原則 | `/Users/fl/Python/oab/docs/omp-bot-control.md`(2026-08-20) |
| 魔術關鍵字規則 | `omp://magic-keywords.md` |
