# 编排者反馈第二轮：worktree 直接开卡、handoff、按实例关闭、报告接收确认（2026-11-28）

任务说明：`~/.deck/drops/1795841650-deck_orchestrator_round2_implementation_prompt_20261128.txt`。
基线：main `6de23a0a`（0.8.45 之后两个 reentry 提交）。运行中的生产 Deck 为 0.8.45（`592f7d01`）。

## 完成时的预期

按使用场景：

1. **外置盘上的 linked worktree 直接开卡（只设一次）。** 在 设置 › MCP › 客户端「Permissions…」勾选「自动允许已授权项目的关联工作树」并保存。之后编排者对 `/Volumes/YottaUnity/seventh-dev45`（或它的子目录，或以后在任何文件夹新建的 worktree）调 `deck_worker_start`，卡片直接开在「第七梦」项目、cwd 就是该 worktree、拿该项目的环境变量。客户端是「所有项目」时，以后新增的项目也自动适用；指定项目模式只覆盖列出的项目。
2. **只认这些项目仓库登记过的 worktree。** 仓库由项目自己的目录决定（主 checkout 的顶层，或本身是 linked worktree 的顶层）；独立 clone、其他仓库的 worktree、submodule、单向 `.git` 指针、链接形式的指针文件、普通目录都开不了卡，错误里 `reason: worktree-unverified` 加 `detail` 说明原因。只授权了仓库子目录的项目不锚定仓库，因此不会扩大到整个仓库。
3. **一个项目多个目录。** 同一对话框里一个项目可以有多行目录，全部显示、全部保存；只改别的权限不会丢目录；外置盘没挂时，已有目录保留并在对话框里提示「目前不可用」。
4. **报告可以写 `status: handoff`。** 工具 schema 里 status 是真正的 enum（含 handoff），未知值报错带 `field` 与 `allowed`。
5. **报告后马上关卡不再反复 EPISODE_MOVED。** 编排者在长等待拿到报告时就决定关闭，用 `deck_worker_close close_mode: "instance"`（带列表里的 `instance` 和 `lastLine`）；工人随后自己结束回合（新 episode）不会再让关闭失效。旧的 `after_episode` 模式照旧。
6. **每次关闭被拒都说清楚。** 前置检查拒绝带 `stage: precheck, accepted: false, observed{…}`；接受后在 Board 事务里被拒，`deck_operation_get` 的 `result.refusal` 给出 `stage: admission`、码、当时观察到的 episode/状态/实例/最新行和下一步。
7. **卡片看得出「最新报告待编排者接收」。** 工人报告后，卡片标签显示「最新报告待编排者接收 {时间}」；编排者用 `deck_worker_reports_ack` 按收到的报告序号确认后，恢复「已报告 {时间}」；再来新报告又显示待接收。

仍然做不到的：

- 开关只扩展开工人卡和托管 session 的 cwd；结构化读取（`deck_project_list/read/search`）、执行许可、stdin、输出共享、会话控制和卡片管理都不变，`authorizedWorkspaces` 仍只列项目目录。
- Git 双向登记是一致性检查，不是签名：能同时改写两边元数据的本机程序可以造出一致的记录；这不是 OS 沙箱。
- Deck 不建、不删、不切 worktree，也不跑 git；只认 `git worktree add` 写出的标准布局（`.git` 文件 ↔ `worktrees/<id>/gitdir` + `commondir`，绝对或相对路径）。项目根是 submodule 或 bare 仓库时不作为锚点（`unsupported-layout` / `not-a-repository-root`），`$GIT_DIR` 被移走的布局也不支持。
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

### A：项目多目录与关联工作树开关（2026-11-28 收敛）

- `ProjectScope.roots` 原本就支持多个；前端改为一目录一行（`initialScope`/`scopePayload` 合并回同一项目），后端 `validate_client_scopes` 合并重复项目、规范化后去重（符号链接与目标算一个），并把「客户端已持有、此刻解析不了」的目录原样保留（外置盘未挂）；新目录必须存在。
- 原先未发布的逐项目「仓库 + 父目录」规则（`Client.worktrees`、`WorktreeRule`、预填按钮与 `mcp_worktree_suggest`）已删除，没有迁移或回退路径。改为 `Client.auto_worktrees`（默认关）。
- 仓库锚点（`grants::repository_anchor`）只来自客户端此刻可达项目的根目录：根的 `.git` 是目录 → 该目录；根本身是 linked worktree → 双向校验后取其仓库 Git 目录；子目录根、submodule、bare 不锚定。每次使用时重新读取，项目默认目录改了就用新目录。
- 候选目录（`grants::auto_worktree_scope`）：最近的 `.git` 必须是文件，指向 `<common>/worktrees/<id>`，`commondir` 回到 `<common>`，`gitdir` 回指；`<common>` 等于某个项目的锚点。worktree 顶层还要通过 `mcp_fs::root_policy`（拒绝 `/`、home 及其祖先、敏感目录），不再有父目录白名单。指针文件以 `O_NOFOLLOW` 打开，在打开的描述符上检查是常规文件且 ≤4 KiB。
- 普通 roots 先判；只有 roots 不包含时才查开关。同一仓库属于多个项目 → `PATH_AMBIGUOUS`，`project_id` 只在合法候选里选。
- 接受时计划里记录 `cwdGrant`（`roots` / `auto-worktree`）和 cwd 的 (dev, ino)。

### A follow-up：实际启动前的最终准入

- `commands::admit_launch_locked`：在 delivery 锁 + 一次账本写事务内，于首次原生副作用之前执行。托管 session 在 `mcp_start_session` 的计划读取段内调用；工人卡由 `mcp.js` `startWorker` 把 `mcpOperation` 交给普通 `start_session`，在其 worker 线程里、`start_session_gated` 之前调用。
- 检查：operation 仍为 executing 且类型对；目录、命令、项目与计划一致；客户端未撤销、仍有 workers / create 权限、无应急 fence；项目仍在可达范围；`project_cwd` 按此刻的 roots 与开关重新授权（仓库登记重新读取）；目录身份 (dev, ino) 未变。结果写入 `result.launch`（`admittedAt` + `grant`，或 `refusedAt` + `reason`）；拒绝时 operation 置 `rejected`、码 `launch-refused`，webview 据 `mcp-launch-refused` 报 rejected 而非 ambiguous。
- 时序：保存权限与准入都取 delivery 锁，互斥。关开关时，尚未被 Board 取走的依赖开关的 start/create 直接 `auto-worktrees-withdrawn`；已取走但未通过准入的在准入处被拒；已通过准入的按自身结果完成；已运行的卡不受影响；撤销 client/项目/workers/create 仍走原有更强路径。
- 同一 `request_id` 的精确重放现在先于目录判定（之前权限变化后重放会得到新拒绝），始终返回原 operation。

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

- mcp.json：handoff 或确认字段 → sticky v22（`STATE_VERSION_REPORT_ACKS`）；关联工作树开关 → v23（`STATE_VERSION_AUTO_WORKTREES`；v23 从未在任何已发布版本里出现过）。本版本起，读取前先只读版本号，未来版本明确报「written by a newer Deck」，不再走成员解析失败。旧版（v21 读者）遇到 v22/v23 会拒绝整个账本（MCP 不可用、文件不动）；回退前需注意。
- 关闭计划的新字段在 `result` 里，旧版读到也不会执行（重启后都是 ambiguous）。
- 新工具 `deck_worker_reports_ack`、新参数 `close_mode`/`instance`/`last_line`、视图新字段对旧客户端无害。客户端需要重新 `tools/list` 才看得到：Claude Code / Codex 新会话自动；隧道等长驻 adapter 需重启集成（Settings 的 MCP 行在协议号不同时才会提示，本轮协议号未变）。

## 可复制示例

见 `docs/mcp.md` 工具表与本目录 `examples.md`。

## 验证（2026-11-28 收敛为开关之后，最终代码）

| 项 | 地点 / 构建 | 结果 |
|---|---|---|
| fmt、clippy（CI 形式） | 本机 | 通过 |
| CI 同款 llvm-cov | 本机，隔离 HOME/TMPDIR | exit 0；deck-app 1588 通过 0 失败；行 85.16%、函数 82.86%；TMPDIR 无遗留 |
| deck-mcp 单元 3 + stdio 8；IPC 普查、EDR 测试 | 本机 | 通过 |
| `a_linked_worktree_belongs_to_the_project_whose_root_anchors_its_repository`（不同位置 worktree、子目录、clone、其他仓库、submodule、单向指针、链接指针、超长指针、开关关闭、子目录根不扩大、linked-worktree 根锚定、双项目歧义、roots 优先） | 本机 | 通过 |
| `the_worktree_switch_is_decided_again_at_the_launch`（所有项目模式新增项目自动适用、指定项目不含未列项目、歧义消歧；claim 后关开关被拒并记录、未 claim 的被 `auto-worktrees-withdrawn`、roots 授权的不受影响；已准入的照常完成；精确重放返回原 operation、改参冲突；登记被移除、目录被替换、计划外目录、项目被删、workers 权限撤销；托管 session 同样被挡） | 本机 | 通过 |
| `a_project_keeps_every_directory_it_was_given` | 本机 | 通过 |
| `scripts/ui-tests` | 本机 | 1069/1069 |
| air2 `worktree-switch` → 重启 → `worktree-switch-relaunch`，真实 WKWebView 窗口、真实 Permissions… 对话框勾选保存、真实 debug adapter（run `v134f94f9`） | air2 | **PASS** 阶段一 3/3、阶段二 4/4，客户端 7/7：重启后开关仍开；重启后才建的项目「Later」与两个不同文件夹的 worktree（`git worktree add`，开关保存之后才创建）直接开卡，项目与 cwd 正确，`result.launch.grant = auto-worktree`；clone 被拒 `separate-repository`；普通目录 `outside-scope`；`authorizedWorkspaces` 不含 worktree |
| air2 `worker-close` 回归（run `vedf4bc47`，工人卡经最终准入启动） | air2 | **PASS** 6/6，客户端 15/15 |

以下为收敛前（逐项目规则版本）的记录，保留作历史：

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

- 多目录行与「外置盘不可用」提示在真实窗口里的外观与操作（只有 DOM 级单元测试）。
- 项目环境变量随 worktree 卡生效：代码路径与普通卡相同（`start_session` 的 `projectId`），但端到端没有写 Keychain 去验证。
- 真实外置盘 `/Volumes/YottaUnity` 上的 linked worktree 开卡（单元测试用同样布局的临时目录；没有在生产 Deck 上改用户配置）。
- 实际出错编排者的 adapter 版本与工具缓存；Codex、Pi 编排者调用新参数；发布包。
- 旧版 Deck 读到 v22/v23 账本的实际表现（按代码推断为整体拒绝，未运行旧版）。
