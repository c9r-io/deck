# 编排者反馈第二轮：worktree 直接开卡、handoff、按实例关闭、报告接收确认（2026-11-28）

任务说明：`~/.deck/drops/1795841650-deck_orchestrator_round2_implementation_prompt_20261128.txt`。
基线：main `6de23a0a`（0.8.45 之后两个 reentry 提交）。运行中的生产 Deck 为 0.8.45（`592f7d01`）。

## 完成时的预期

按使用场景：

1. **外置盘上的 linked worktree 直接开卡。** 在 设置 › MCP › 客户端「Permissions…」的「Git 关联工作树」里，为项目（如「第七梦」）填仓库主 checkout（`/Users/chenhan/UnityProjects/architect-of-the-seventh-dream`）和工作树所在文件夹（`/Volumes/YottaUnity`），保存一次。之后编排者对 `/Volumes/YottaUnity/seventh-dev45`（或它的子目录，或以后新建的 `seventh-devNN`）调 `deck_worker_start`，卡片直接开在「第七梦」项目、cwd 就是该 worktree、拿该项目的环境变量。不再需要先在主仓库开卡再 cd。
2. **规则只认这个仓库的 worktree。** 同一文件夹里的独立 clone、其他仓库的 worktree、submodule、伪造的单向 `.git` 指针、普通目录都开不了卡，错误里 `reason: worktree-unverified` 加 `detail` 说明原因。
3. **一个项目多个目录。** 同一对话框里一个项目可以有多行目录，全部显示、全部保存；只改别的权限不会丢目录；外置盘没挂时，已有目录保留并在对话框里提示「目前不可用」。
4. **报告可以写 `status: handoff`。** 工具 schema 里 status 是真正的 enum（含 handoff），未知值报错带 `field` 与 `allowed`。
5. **报告后马上关卡不再反复 EPISODE_MOVED。** 编排者在长等待拿到报告时就决定关闭，用 `deck_worker_close close_mode: "instance"`（带列表里的 `instance` 和 `lastLine`）；工人随后自己结束回合（新 episode）不会再让关闭失效。旧的 `after_episode` 模式照旧。
6. **每次关闭被拒都说清楚。** 前置检查拒绝带 `stage: precheck, accepted: false, observed{…}`；接受后在 Board 事务里被拒，`deck_operation_get` 的 `result.refusal` 给出 `stage: admission`、码、当时观察到的 episode/状态/实例/最新行和下一步。
7. **卡片看得出「最新报告待编排者接收」。** 工人报告后，卡片标签显示「最新报告待编排者接收 {时间}」；编排者用 `deck_worker_reports_ack` 按收到的报告序号确认后，恢复「已报告 {时间}」；再来新报告又显示待接收。

仍然做不到的：

- worktree 规则只作用于开工人卡和托管 session 的 cwd；结构化读取（`deck_project_list/read/search`）仍只在项目目录里。
- Deck 不建、不删、不切 worktree，也不跑 git；只认 `git worktree add` 写出的标准布局（`.git` 文件 ↔ `worktrees/<id>/gitdir` + `commondir`，绝对或相对路径）。bare 仓库为主的布局、`$GIT_DIR` 被移走的布局不支持，会给出 `unreadable-metadata` 或 `other-repository`。
- 「待接收」只说最新一份报告是否被确认收到，不是未读条数，也不表示已审阅或已处理。升级前就有报告的旧卡显示原来的「已报告」，直到下一份新报告。
- 按实例关闭要求 Deck 读得到前台进程；读不到（`INSTANCE_UNKNOWN`）或活着的 agent 没有观察（`OBSERVATION_UNKNOWN`，如 Deck 重启后尚未见新回合）时拒绝，此时可退回 episode 模式。
- 旧版 Deck（≤0.8.45）读到 v22/v23 的 mcp.json 会拒绝（MCP 在旧版里不可用），回退版本前要知道这一点。

## 根因与现场证据

- **A（PATH_NOT_ACCESSIBLE）**：机制已确认。生产 `mcp.json`（只读查看）里编排客户端为 `allProjects`；「第七梦」默认目录 `/Users/chenhan/UnityProjects/architect-of-the-seventh-dream`，其 worktree 在 `/Volumes/YottaUnity/seventh-*`（`.git` 文件指向主仓库 `.git/worktrees/<id>`，`gitdir` 回指、`commondir` = `../..`，标准布局）。这些目录不在任何项目根之下，`start_project` 只做路径包含判断，所以必然 `PATH_NOT_ACCESSIBLE`。生产 app.log 有一条 `refused deck_worker_start code=PATH_NOT_ACCESSIBLE`（1795837814），日志不记录 dir，**原始调用的精确 dir 未唯一定位**；外置盘当时是否挂载也无法从日志确认。
- **B（handoff 被拒）**：机制已确认（`STATUS_WORDS` 五个值、adapter 只是 string）。app.log 有 `refused deck_worker_report code=INVALID_ARGUMENTS`（1795833776），日志不记录字段，**是否正是 handoff 未唯一定位**。
- **C（EPISODE_MOVED）**：现场证据较强。app.log 里 13 次 `refused deck_worker_close code=EPISODE_MOVED`，全部是**前置检查**阶段（路由层拒绝，没有被接受后在 admission 拒绝的记录），其中多数紧跟在某张卡 `turn-done` 之后 1–14 秒（例如 1795833799 `turn-done s=sess-ad984` → 1795833805 拒绝；1795839859 → 1795839860）。episode 编号 e=20…130 由全局分配器发放、跳号。与「工人在 working 时报告 → 长等待返回 → 编排者按该 episode 关闭 → 工人正常 turn-done 换号 → 关闭被拒」一致；单元测试和 air2 真实端到端都按这个时序确定性复现（见下）。日志不记录卡与 episode 的对应，**单次事件对应哪张卡未定位**。未发现「同一交互同一状态重复观察换号」：代码上相同 (interaction, word) 保持 episode，测试覆盖。
- **D**：功能缺口，无事故。
- adapter：本机生产 adapter 由 0.8.45 打包；出错编排者实际用的 adapter/工具缓存**未取得**。Claude Code 在每次会话开始时重新 `tools/list`；长期运行的隧道 adapter 跨 Deck 更新仍是旧二进制，协议号未变时不会被拒，所以新工具要重启集成后才出现（本轮未提升 `CONTROL_PROTOCOL`：新增字段与工具对旧 adapter 无害，旧 adapter 只是看不到它们）。

## 关键设计

### A：项目多目录与 worktree 规则

- `ProjectScope.roots` 原本就支持多个；前端改为一目录一行（`initialScope`/`scopePayload` 合并回同一项目），后端 `validate_client_scopes` 合并重复项目、规范化后去重（符号链接与目标算一个），并把「客户端已持有、此刻解析不了」的目录原样保留（外置盘未挂）；新目录必须存在。
- 新增 `Client.worktrees: [{projectId, repo, parent}]`（保存时规范化：repo 必须含 `.git` 目录，parent 走 root policy）。两种 scope 模式都可用；规则的项目必须在客户端可达范围内，项目离开范围时规则随之删除。
- 选择了「仓库限定」而非「专用父目录」：父目录只是路径边界。`grants::verify_linked_worktree` 在进程内有界读取（每个指针文件 ≤4 KiB，`.git` 文件不跟随符号链接，无子进程）：目标的最近 `.git` 必须是**文件**，指向 `<repo>/.git/worktrees/<id>`；该目录的 `commondir` 解析为 `<repo>/.git`；其 `gitdir` 回指这个 `.git` 文件。remote、分支、HEAD、目录名都不读。
- 候选项目 = 根目录包含 dir 的项目 ∪ 规则验证通过的项目；多个 → `PATH_AMBIGUOUS`，`project_id` 只能在候选里选。写入阶段在账本写锁内用当时的客户端再判一次（撤销与开卡竞争）。失败不回退到主仓库目录。错误 `reason`：`not-found`、`volume-unavailable`、`not-accessible`、`outside-scope`、`worktree-unverified`（`detail` 为五个闭集词之一）。
- 同一规则也用于 `deck_session_create` 的 cwd（`grants::project_cwd`）。EDR：未新增任何 spawn。

### B：handoff

`STATUS_WORDS` 加 `handoff`；adapter 用 `schema_with` 输出 `{"type":"string","enum":[…]}`，值仍原样转给 Deck，未知值由后端给出 `field`、`allowed`。两端都对 `mcp-fixtures/report-status.json` 做契约测试；stdio 集成测试在真实 adapter 进程的 `tools/list` 里检查 enum。

### C：关闭模式、实例绑定与线性化点

- `close_mode: episode`（默认，原语义）/ `instance`（新增）。实例 = 卡片 Signal 目标 pane 的 (tmux server pid, session id, pane id, pane pid) 键 + 前台进程代际（pid + 出生时刻）键，视图里给出 `instance` 令牌和 `lastLine`（最新被接受的行）。与 attention episode、lane_epoch 互不替代。
- instance 模式拒绝：`NEW_INPUT`（决定后又接受了行）、`LINES_PENDING`（还有未送达的行）、`INSTANCE_CHANGED`（换了程序或 session）、`INSTANCE_UNKNOWN`（读不到前台/队列）、`OBSERVATION_UNKNOWN`（活着的 agent 没有观察，无法证明没在问用户）；`WORKER_NEEDS_YOU`、`WORKER_TAKEN_OVER` 与原来相同。同一 pane 前台回到 shell、或 session 已不在列表中，视为实例已结束，允许关闭（走原清理路径）。
- 顺序：关闭在账本写锁内被接受后，到它结束前，该卡的 send/interject 一律 `WORKER_CLOSING`（带 `closeOperationId`），第二个关闭 `CLOSE_IN_PROGRESS`。接受前进入的新行由 `lastLine` 比较拒绝关闭。**线性化点**是 `close_admit`（delivery 锁下、Board 关闭事务内）：在它之前的变化（turn 结束不算、换实例/新行/未送达行/needs-input/接管/撤权算）决定结果；admitted 后由 webview 的普通关闭执行，仍可能因用户自己的保护（pane 正在显示、Reminder、保留的便笺）被拒。
- 计划里持久化 `closeMode`、`instance`/`lastLine` 或 `afterEpisode`、`workerOperationId`；request hash 覆盖新参数（旧请求的序列化不变）。重启不重放（原机制：accepted/executing/admitted → ambiguous）。

### D：最新报告接收确认

`Worker.acknowledged: Option<u64>`：`None` 不跟踪（无报告、旧记录、无主卡），有编排者的卡在升级后的第一份新报告的同一写事务里变为 `Some(0)`；`deck_worker_reports_ack {card_id, sequence}` 只接受内核证明的所属编排卡、需 replies 权限，`1..=reported`，取最大值（不回退、可重放），不唤醒任何等待，只发 `mcp-workers-changed` 给界面。`WorkerCardView.report_pending` 只是布尔；交回（变成无主）时清为 `None`；随 worker 记录 prune。

## 兼容与升级

- mcp.json：handoff 或确认字段 → sticky v22（`STATE_VERSION_REPORT_ACKS`）；worktree 规则 → v23（`STATE_VERSION_WORKTREES`）。本版本起，读取前先只读版本号，未来版本明确报「written by a newer Deck」，不再走成员解析失败。旧版（v21 读者）遇到 v22/v23 会拒绝整个账本（MCP 不可用、文件不动）；回退前需注意。
- 关闭计划的新字段在 `result` 里，旧版读到也不会执行（重启后都是 ambiguous）。
- 新工具 `deck_worker_reports_ack`、新参数 `close_mode`/`instance`/`last_line`、视图新字段对旧客户端无害。客户端需要重新 `tools/list` 才看得到：Claude Code / Codex 新会话自动；隧道等长驻 adapter 需重启集成（Settings 的 MCP 行在协议号不同时才会提示，本轮协议号未变）。

## 可复制示例

见 `docs/mcp.md` 工具表与本目录 `examples.md`。

## 验证（逐项，地点与构建）

| 项 | 地点 / 构建 | 结果 |
|---|---|---|
| `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings` | 本机，最终代码 | 通过 |
| CI 同款 `cargo llvm-cov --workspace --all-targets --all-features --locked --offline --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` | 本机，隔离 HOME/TMPDIR | exit 0；deck-app 1587 通过 0 失败（14 ignored），行 85.16%、函数 82.80%；TMPDIR 无遗留 |
| deck-mcp（adapter）单元 3 + stdio 集成 8（真实 adapter 进程的 `tools/list`：40 个工具、status enum 含 handoff、close_mode enum、ack 工具） | 本机 | 通过 |
| 新增 Rust 测试：`a_close_bound_to_its_instance_survives_the_workers_own_turn_ending`（C1 时序的确定性复现、两种模式、两阶段诊断、CLOSE_IN_PROGRESS / WORKER_CLOSING、换实例/换 session/未知/新行/未送达/结束实例、重放与改参冲突）、`reports_say_handoff_and_their_orchestrator_acknowledges_each_by_sequence`、`a_ledger_from_a_newer_deck_is_refused_by_its_version_untouched`、`a_linked_worktree_is_verified_both_ways_from_the_repositorys_own_files`、`a_worktree_rule_lets_later_worktrees_start_worker_cards_in_their_project` | 本机 | 通过 |
| `scripts/ui-tests` | 本机，最终代码 | 1067/1067（完整并行跑时一个与本轮无关的计时用例 `CJK prose … bounded scan` 偶发超 1 s，单独跑通过） |
| `check-js-syntax`、`check.mjs`、`extension_catalog.py --check` | 本机 | 通过 |
| air2 WKWebView `worker-close`，真实 Claude Code 2.1.332 工人（debug 载体，隔离根目录，登记表清理） | air2，run `v8abd3b11` | **PASS** 6/6 检查点；客户端 15/15：真实 `tools/list`；报告后 31 s 内一次长等待返回（working、episode 1、handoff）；确认 → 标签由待接收恢复（b=3）；未来序号 `REPORT_NOT_FOUND`；工人自然 turn-done（episode 2，同一实例）；episode 模式关闭 `EPISODE_MOVED`（precheck，current 2）；instance 模式关闭 `committed`；精确重放同一 operation |
| 同上第一次 | air2，run `v926bc22e` | 功能流程相同且账本显示 ack=1、关闭 committed；判定 FAIL 是测试脚手架缺陷（标签检查点的 a 值写成 0；页面过早关闭编排卡导致客户端证据未写出），已修后重跑即上一行 |
| 已有 re-entry 用例 `a_client_held_by_its_own_exit_question_is_hung_up_and_reentered_in_its_own_session` | 本机 | **偶发失败，与本轮无关**：未修改的 HEAD `6de23a0a` 上 5 次失败 1 次，本轮代码上 5 次失败 2 次（负载下被杀的 tmux server 给未勾选卡的替身发 SIGHUP，替身写出 `hup` 行）；CI 同款门禁里它通过 |

**UNRUN / 未验证：**

- 真实窗口里在 Permissions… 对话框中添加 worktree 规则、多目录行与「外置盘不可用」提示的外观与操作（只有 DOM 级单元测试）。
- 真实外置盘 `/Volumes/YottaUnity` 上的 linked worktree 开卡（单元测试用同样布局的临时目录；没有在生产 Deck 上改用户配置）。
- 实际出错编排者的 adapter 版本与工具缓存；Codex、Pi 编排者调用新参数；发布包。
- 旧版 Deck 读到 v22/v23 账本的实际表现（按代码推断为整体拒绝，未运行旧版）。
