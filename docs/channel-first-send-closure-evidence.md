# 频道首步授权：独立复核收尾的证据摘要

日期：2026-10-07。本文件是 `docs/channel-first-send-verification.md`「独立复核收尾」一节的可核查摘要：每行给出场景、测试名、被测对象、退出码、关键断言和判定。原始日志在本机隔离目录 `/private/tmp/claude-501/dk1/logs/`（不入库）；下面的测试都在仓库里，任何人可用同样的命令重跑。

## 被测对象

- 修复前：提交 `418dce8ba96de64941a6c19989e835ccbcc9c89c` 的生产代码，加上本轮新增的测试。F1 保存侧和 F3 需要本轮新增的测试接缝才能运行，因此「修复前」是「接缝已加、行为仍是候选行为」的一次性构建，各行注明。
- 修复后：包含本文件的那次提交的工作树（提交哈希见交付消息）。起始 HEAD 为 `418dce8`，分支 `main`，起始工作树干净。
- 真实 WKWebView 载体的可执行文件 SHA-256：`3abaeaba52bb3942ad3a4e17dad993a71ed756dc31bc714d465ab020210bbe56`（签名后的 bundle 内 `deck`）。载体构建之后没有再改生产代码：之后只改了测试文件、文档和 `scripts/edr_runtime.py`，生产源文件的合并哈希在载体运行后与最终门禁前一致。

## 运行方式

```sh
# 隔离环境：私有 HOME / TMPDIR / ZDOTDIR，固定现有工具链，Cargo 离线
export CARGO_HOME=/Users/<user>/.cargo RUSTUP_HOME=/Users/<user>/.rustup CARGO_NET_OFFLINE=true
export HOME=<empty dir> TMPDIR=<short empty dir> ZDOTDIR=<empty dir>
cd app/src-tauri && cargo test -p deck-app channel_admission_tests   # 本表的 Rust 行
cd app && node --test ui/test/channel-editor-dom.test.mjs ui/test/channel-recovery-dom.test.mjs ui/test/channel-model.test.mjs
```

`TMPDIR` 必须短：测试数据目录在它下面，tmux socket 路径要放得进 `sun_path`。Rust 行里的每个测试都在自己的子进程里运行（inbox、已提交 Board 和 Slack 身份是进程单例），数据目录是该进程私有的 `deck-test-<pid>`；单元测试构建在调用系统 Keychain 之前就返回，凭据只存在于进程内存。没有连接真实 Slack，没有启动真实 Agent。

## 五项发现：同一断言，修复前失败，修复后通过

Rust 测试名省略前缀 `documents::channel_admission_tests::`。

| 发现 | 场景 | 测试 | 修复前（退出码，失败的断言） | 修复后 | 判定 |
|---|---|---|---|---|---|
| F1 载入侧 | 备份 A 含表情规则的首发选择和频道授权，主文件 B 已撤销两者，B 损坏后由所有者入口 `load_settings` 载入 | `settings_recovery_keeps_withdrawn_authority_withdrawn_and_saves_working` | 101。`loading never makes the backup the current settings`：`read_typed` 来源 left `Some("main")`，right `Some("backup")`。载入把整份备份写成了主文件 | 0 | CONFIRMED，已修复 |
| F1 保存侧 | 同一测试的下一步：载入备份后做一次无关保存。「修复前」= 载入侧已修、保存侧读取仍为候选代码 | 同上 | 101。`save_settings` 返回 `current settings are unavailable` | 0 | CONFIRMED，已修复 |
| F2 损坏 | 进程第一次打开 inbox 之前把 `channel-inbox.json` 写成截断的 JSON，再经外部入队入口为表情卡片入队 | `a_damaged_channel_inbox_does_not_block_other_external_sources` | 101。第一条表情行：`DeckError { kind: Recovery, message: "channel inbox is unreadable" }` | 0 | CONFIRMED，已修复 |
| F2 不可读 | 同上，文件权限 000 | `an_unreadable_channel_inbox_does_not_block_other_external_sources` | 101。`DeckError { kind: Perm, message: "channel inbox could not be read" }` | 0 | CONFIRMED，已修复 |
| F3 原生 | 凭据已存、频道连接已开、零条频道规则；真实 `socket_loop` 对回环 Slack fixture。「修复前」= 接缝已加、启动计划仍为候选逻辑 | `the_first_first_send_rule_is_saved_by_one_save_from_zero_rules` | 101。`the transport verifies the identity for the waiting save: "pending"`；此前断言已确认带请求的保存返回错误、规则数为 0 | 0 | CONFIRMED，已修复 |
| F3 界面 | 同一场景经真实编辑器保存处理函数。「修复前」= 候选的 `automation.js` 与 `channel-model.js` | `ui/test/channel-editor-dom.test.mjs`（新增 4 项，改 1 项） | 1。5 项中 4 项失败：新增 3 项（不显示等待、准备调用 0 次、取消后未释放原生等待）；另 1 项是原有测试，本轮把它的期望改成请求带身份，候选代码的请求不带 | 0，5/5 | CONFIRMED，已修复 |
| F4 | 版本 1 的 inbox（`handled` 只有 id 和时间）、三张旧版运行卡片（无 `target`、无 `firstSend`、`initialQueued=false`） | `a_run_acknowledged_by_an_older_deck_finishes_its_plan` | 101。第一步入队：`channel event proof is missing` | 0 | CONFIRMED，已修复 |
| F5 | 模板三步文本完全相同，规则已授权 | `a_follow_up_step_may_repeat_the_head_text` | 101。`step 1: ... "channel first-send claim is missing"` | 0 | CONFIRMED，已修复 |

修复前日志：`before-rust-418dce8.log`、`before-rust-F1-pathB.log`、`before-rust-F3.log`、`before-ui-F3-418dce8.log`。

## 各测试实际断言了什么

**F1**（真实入口 `load_settings` / `save_settings`，真实文件）
1. 载入备份后：交给 webview 的内容没有频道 `firstSend` / `firstSendGrant`；`storage::read_typed` 的来源仍是 `backup`；`inbound::read_config_strict()` 为 `None`，即 B 撤销的表情首发选择没有重新成为当前权威；`inbound_channel::read_config_strict_result()` 为错误。再载入一次结果相同。
2. 此时改模板首步的 Board 保存不被拒绝：`retire_channel_grants_locked` 返回 `false`。
3. 把载入的内容做无关修改后保存成功，返回值和主文件都不含授权；再载入来源为 `main`，频道规则 `first_send=false`、无授权。
4. 页面仍持有旧授权并原样发回：保存成功但授权被去掉。
5. 主文件再次损坏、再次从备份恢复：交回的每条频道规则都没有选择和授权。
6. 恢复之后带明确请求的一次保存：授权签发成功。
7. 从未使用频道首发的设置：恢复后可保存；运行中主文件损坏可保存；损坏且没有备份也可保存。载入备份时没有写主文件。
8. 主文件不可读（权限 000）：两次保存都被拒绝，字节不变；读取方报告「未知」而不是「已撤销」；恢复可读后授权和表情选择原样有效。所有者载入不可读文件沿用其既有契约（搁置并从备份回答，不用默认值），交回内容同样不带频道授权。

**F2**（真实持久化初始化失败，外部入队入口的准入顺序）
- 先断言 `channel_pending()` 报错，证明 inbox 确实初始化失败。
- 表情卡片的 `at` 行和 `chain` 行、手机任务卡片的行、普通卡片的原文外部文本行都入队；reviewed-list 入口对表情卡片通过。入队的 4 行全部 `external=true`，`authority`、`readiness_override`、频道约束全为空。
- 非 Agent 命令（`zsh`）仍被 agent-only 准入拒绝。
- 频道首步：带 claim、省略 claim、reviewed-list 两种写法，全部拒绝。
- 省略 claim 后再改 `operationId`、`mode`、`text`，或把 `cardId` 换成「去掉 origin 但保留 channelRun 的卡片」「origin 改成 slack 但保留 channelRun 的卡片」「Board 上不存在的卡片」：全部拒绝。受约束运行的后续步骤同样等待。
- Board 不是当前版本（recovered）时，表情行也被拒绝；Board 恢复为当前后通过。

**F3 原生**（回环 fixture：`auth.test`、`apps.connections.open`、Socket Mode `hello`；身份没有被直接写入）
1. 零规则时 transport 不请求任何 ticket，身份为空；带请求的保存被拒绝，规则数仍为 0。
2. 一次保存：`prepare_channel_identity` 让 transport 完成握手（ticket 1 次、socket 1 条），返回的摘要等于握手得到的身份摘要；带该摘要的保存创建规则并签发授权。
3. 保存后释放等待并按规则重连；随后一条匹配消息经真实解析、匹配、暂存，暂存事件冻结了该授权，fixture 收到这条 envelope 的 ACK。
4. 连接已建立时再次保存：准备调用在 200 毫秒内返回，ticket 和 socket 计数不变，连接没有重启。
5. 短暂故障（API 返回 500）：第一次 `pending`，期间没有保存任何规则；恢复后同一请求完成，授权签发。
6. 另一个工作区成为已验证身份：摘要不同，用旧摘要保存被拒绝，设置文件字节不变。
7. `hello` 不带 app id：身份始终不发布，保存被拒绝。
8. 等待中取消：返回 `canceled`；之后 Slack 恢复也不再连接，规则数为 0。
9. 等待中关闭频道连接：本次返回 `pending`，下一次返回 `disabled`；没有 App 令牌时返回 `no-token` 且不请求 ticket。

**F3 界面**（真实保存处理函数，合成 IPC）
- 等待期间：编辑器内显示「尚未保存」，保存按钮和读取过的字段禁用，取消按钮可用，没有保存调用；再点一次保存不发起第二次准备。
- `pending` 后自动再问，身份到达后一次保存同时带规则和 `{external, identity}`；编辑器关闭，等待在保存之后释放。
- Slack 一直不应答：准备调用恰好 6 次后停止，没有保存，规则数 0，编辑器和字段保留，勾选保留，状态行说明未保存且未授权。`disabled` / `no-token` / 其他错误各只问一次。
- 取消按钮和关闭抽屉：原生等待被释放；之后才到达的身份不触发保存。
- 身份与请求不符：原生保存拒绝，没有留下规则。
- 不开启首发的保存不调用准备接口。
- `ui/test/channel-model.test.mjs`：等待的次数上限、只重试 `pending`、取消后丢弃迟到结果。

**F4**
1. 已 ACK、完全未入队：两步都入队。
2. 已 ACK、首步在升级前已入队：首步重放不新增，只补第二步，共 2 行。
3. 首步已发送（队列行已离开、operation 记录保留）：首步重放不新增，只补第二步，共 1 行。
4. 以上所有行 `external=true`，无频道约束、无 `authority`、无 `readiness_override`，尽管规则在升级后已开启首发并持有有效授权。
5. 旧运行附带 claim 请求新权限：拒绝。
6. 新路径运行（暂存时冻结了授权）：省略 claim 拒绝；Board 上的冻结 claim 也删掉仍拒绝；事件被消费（`handled` 记录保留授权引用）后仍拒绝；换 `operationId` 仍拒绝。
7. 原生完全没有记录的事件：拒绝。
- 前端一半：`ui/test/channel-recovery-dom.test.mjs` `a run acknowledged by the previous release replays its old requests and finishes`，重放请求与旧版参数逐字段相同、不带 `channelFirstSend`，只新增缺失步骤，不重复建卡，不再 ACK。

**F5**
- 三步同文：三行入队；首行 `at` 且约束 `authorized=true`，后两行 `chain`、无约束、无授权；三个 operation 互不相同；三步各自重放都不新增。
- 仍然拒绝：首步去掉 claim 并改成 `chain` / `tplIdx=2`；首步省略 claim；后续步骤附带 claim；后续步骤改 `operationId`、`tplIdx`（3 或 1）、`mode=at`、文本、session、命令。

## 最终门禁（修复后的工作树，实际运行）

| 门禁 | 结果 | 日志 |
|---|---|---|
| `node --check`（被跟踪的 JS / MJS，不含 vendored xterm，142 个文件） | 通过 | `final-node-check.log`（空） |
| `scripts/ui-tests` | 575 项通过，0 失败；覆盖率阈值与清单通过 | `final-ui.log` |
| `node ui/js/check.mjs` | 58 个模块通过 | 同上 |
| `cargo fmt --check` | 通过 | `final-fmt.log` |
| `cargo clippy --workspace -- -D warnings` | 通过 | `final-clippy.log` |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略 | `final-cargo-test.log` |
| `python3 scripts/test_edr_runtime.py` | 7 项通过 | 终端输出 |

两项忽略与候选报告相同：一项性能测试，一项需要已认证真实 Claude 的测试。候选报告的 1,183 项加本轮 6 项 Rust 测试为 1,189；UI 569 项加 6 项为 575。

原有验收没有逐项重新设计：它们随 `cargo test --workspace` 和 `scripts/ui-tests` 全部重跑并通过，包括 `native_channel_admission_binds_source_scope_frozen_intent_and_retirement`、`scheduler::channel_permission_tests` 全部、`tests/external_admission.rs`、`tests/signal_census.rs`、`tests/edr_quiet.rs`、`tests/session_architecture.rs`。为本轮改动更新的固定断言只有两处，见报告。

## 真实 WKWebView 后台完整链路（修复后的生产代码）

- 层级：真实隔离 WKWebView + 合成 Slack envelope + 无害测试 Agent（bundle 内名为 `claude` 的 fixture）。Slack 身份由调试专用的 `channel_smoke_identity` 注入，**不经过** transport；它不替代 F3 的零规则首次配置测试，F3 也不替代它。
- 数据目录 `/tmp/deck-channel-ndzAYRB6`，tmux socket `deck-smoke-channel-1791343973-22426`，独立 bundle `deck-channel-smoke-1791343973-22426.app`，以 `open -g` 后台启动。
- `scripts/smoke-verdict /tmp/deck-channel-ndzAYRB6 channel-first-send`：退出码 0，`PASS (9/9 expected checkpoints, 9 lines)`。
- 时间线（epoch 秒，取自该目录的 `app.log`）：确认隐藏 `1791343997`；会话建立 `1791344246`，距隐藏 249 秒；首步兼容等待 `1791344251`–`1791344267`；首步发送 `1791344268`（25 字节，`first send without readiness confirmation`）；原生观察结束并由页面报告 `1791344345`，距首发 77 秒。
- `channel-smoke-result.json`：`waitedSecs=347`，`delivered=true`，`held=true`，`receipts=1`，`persistedItems=1`，`persistedDeliveries=1`，`persistedOverrides=1`。第二步在观察期内保持等待。
- 隐藏等待期间没有前端轮询、截图、激活或输入；没有放宽 Agent、Signal 或粘贴门槛。

## 资源清理

- 本轮自有进程：应用 `23159`、tmux 服务器 `23172`、fixture `23365`。应用以 `SIGTERM` 停止；tmux 服务器和 fixture 由 `scripts/edr_runtime.py --cleanup --include-foreground --socket deck-smoke-channel-1791343973-22426` 停止，退出码 0，之后三个 PID 都不存在。bundle 移入 `~/.Trash/Deck-channel-closure-1791344373/`。隔离数据目录保留作证据。
- 没有清理默认 `deck` / `deck-dev` socket、`~/.deck`、Applications 安装、剪贴板或 Keychain。
- **未触碰的遗留**：候选阶段七次载体运行的 tmux 服务器仍在运行，PID `34924 39778 53461 59450 72288 85868 97953`，socket 为 `deck-smoke-channel-17913350…` 至 `…1791338522-95938`。原因见报告：当时的清理工具不认识这种 bundle 名，退出码为 0 但什么也没停。它们不是本轮创建的资源，没有处理。
- **后续（2026-10-07，经用户授权）**：七个服务器已用 `deck-smoke.app` 自带的 `tmux` 3.7c 按精确 socket 名 `kill-server` 结束，退出码均为 0；上述七个 PID 和六个 pane shell PID `46656 53572 65064 76988 93960 7141` 之后都不存在，不带 `--socket` 的清点返回空、退出码 0。失效 socket 文件和 `/tmp/deck-channel-*` 数据目录保留。

## F3.1 异步编辑身份收尾（2026-10-07）

### 被测对象

- 起始 `main` @ `66f8874`，工作树干净。生产改动两个文件：`app/ui/js/automation.js`、`app/ui/js/settings.js`。新测试 `app/ui/test/channel-editor-race-dom.test.mjs`。
- “修复前”是把这两个文件临时还原为 `66f8874` 的内容（`git stash push` 仅这两个路径，运行后 `git stash pop`），用**最终的测试文件**运行。没有一次性构建，没有接缝。
- 运行命令：`node --test --test-timeout=8000 test/channel-editor-race-dom.test.mjs`（在 `app/ui` 下）。隔离的 HOME/TMPDIR/ZDOTDIR 与前文相同。

### 修复前实际写入了什么

在 `66f8874` 上执行报告的序列（编辑 A → 保存 → 等待 → 打开 B → A 的身份返回），读取唯一一次 `save_settings` 的参数：

```
channel  written rules: []  channelRules: ['ra']  requests: [{"ruleId":"ra","external":true,"identity":"verified-identity"}]  editor hidden: false
clock    written rules: []  channelRules: ['ra']  requests: [{"ruleId":"ra","external":true,"identity":"verified-identity"}]  editor hidden: false
slack    written rules: []  channelRules: ['ra']  requests: [{"ruleId":"ra","external":true,"identity":"verified-identity"}]  editor hidden: false
```

三种情况下 `rb` 都从写入的设置里消失。

### 同一断言，修复前后

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `a save waiting for Slack is dropped when another rule (channel) is opened: that rule is untouched` | 失败 `nothing was saved`，`1 !== 0` | 通过 |
| 同上 `(clock)` | 失败，`1 !== 0` | 通过 |
| 同上 `(slack)` | 失败，`1 !== 0` | 通过 |
| `a late failure of a dropped save says nothing in the editor that replaced it` | 失败 `assert.ok(editorFree())` | 通过 |
| `a save waiting for Slack is dropped when the editor for a new rule is opened` | 失败 `assert.ok(editorFree())` | 通过 |
| `a canceled save that succeeds late leaves the next save's wait alone` | 失败 `the new save is still held and shown as waiting` | 通过 |
| `a canceled save that fails late leaves the next save's wait alone` | 失败，同上 | 通过 |
| `the next save is still cancelable after an older save's answer arrived` | 通过 | 通过 |
| `a save queued behind another settings write is not written once its edit is gone` | 失败 `the queued save was not written`，`2 !== 1` | 通过 |
| `a waiting save that goes ahead keeps what was saved meanwhile and never revives a deleted rule` | 失败 `only the deletion was written`，`2 !== 1` | 通过 |
| `losing focus or being covered does not give a waiting save up` | 通过 | 通过 |
| `a save whose approval is still being computed is dropped when another rule is opened` | 失败，`1 !== 0` | 通过 |
| `pausing a rule from the list never replaces the rule open in the editor` | 失败 `the rule being edited was not removed` | 通过 |
| `a save already sent finishes as its own: it closes and reports to no other editor` | 通过 | 通过 |

合计：修复前 `pass 3 / fail 11`，退出码 1；修复后 `pass 14 / fail 0`。

### 各测试的层级与关键断言

- 层级：node 下的测试 DOM；生产的 `initAutomation` 事件接线、保存按钮处理函数、`openAutomations` 绘制出的规则行上的编辑与暂停按钮、`saveRule`、`persistInbound` / `commitSettings` / 设置写入队列全部是生产代码。合成的只有 `window.__TAURI__.core.invoke`：`slack_channel_prepare` 按脚本回答，`save_settings` 可被测试扣住再放行，并只为请求里点名的规则写入授权。
- 迟到结果：`slack_channel_prepare` 返回测试持有的 promise。取消或切换之后测试才 `resolve` / `reject` 它，所以旧回调确实在失效之后运行。
- 切换后立即断言新编辑器可用（控件未锁、无等待提示、字段是 B 的值）；旧结果返回后再断言 `save_settings` 0 次、规则集合与事前快照 `deepEqual`、编辑器仍是 B 的；随后在 B 的编辑器里保存一次，只改变 B，A 与事前完全相同。
- 取消 A 后保存 C：A 的结果返回时，C 仍显示等待、控件仍锁定、提示文本不变、`slack_channel_prepare_cancel` 仍是 1 次；C 的身份返回后恰好一次保存，授权请求的 `ruleId` 是 C，A 没有 `firstSend`。
- 排队后失效：先让一次无关的 `persistSettings()` 占住写入器并扣住它的 IPC，A 的身份立即返回后入队，然后打开 B，再放行。`save_settings` 总数 1（只有无关的那次），授权请求为空，内存中的规则与事前相同。
- 已发出的写入：扣住 A 自己的 `save_settings`，期间打开 B，再放行。写入完成且只有 1 次，A 得到授权，B 的内容与事前相同，B 的编辑器没有被关闭，没有新的提示，原生保持在写入后释放 1 次。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS 加新测试 | 通过 |
| `scripts/ui-tests` | `tests 589 / pass 589 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略 |

本轮没有构建应用，没有二进制摘要。

### 没有运行的

245 秒后台 WKWebView 载体、fmt、clippy、`test_edr_runtime.py` 没有重跑，理由见报告。没有启动应用或 tmux 服务器，没有创建需要清理的进程、socket 或 bundle。

## F3.2 撤回候选的跨写入隔离（2026-10-07）

起始 `main` / `795bebc` / 工作树干净。生产改动：`app/ui/js/settings.js`（写入器），`app/ui/js/automation.js`（头部契约一句）。测试：`app/ui/test/channel-editor-race-dom.test.mjs` 新增 8 项。

### 修复前的实际写入（`795bebc` 的 `settings.js`，同一测试场景）

设置里有频道规则 `ra`、Clock 规则 `rb`（目录 `/tmp`、每天 09:00）、表情规则 `rs`（目录 `/tmp`、无 `autoSend`）。顺序：W0 `setShortcut` 已发出未返回 → A 编辑器保存 → B `setFontScale(1.2)` → C `setShortcut` → 取消 A → 放行 W0，B 落盘后扣住它的响应 → 放行 B → 放行 C。

A 为 Clock 规则 `rb`（目录改 `/var`、时间改 18:30、勾选免就绪首发）：

| 时刻 | `save_settings` 次数 | 该次写入里的 `rb` | 主文件里的 `rb` |
|---|---|---|---|
| W0 返回、B 已落盘未返回 | 2（W0、B；A 没有自己的写入） | `dir:/var, minute:1110, firstSendWithoutReadiness:true` | 同左 |
| B 返回、C 发出 | 3 | `dir:/tmp, minute:540` | |
| C 落盘 | 3 | | `dir:/tmp, minute:540` |

A 为表情规则 `rs`（目录改 `/var`、勾选自动发送）：B 的写入和当时的主文件里 `rs` 为 `dir:/var`，并带有 `autoSend`（键 `classes, digest, external, steps`）；C 的写入和最终主文件恢复为 `dir:/tmp`、无 `autoSend`。两种情况授权请求列表都为空。

C 写入失败的场景：主文件只收到 W0 和 B 两次写入，停在 B 的内容上，`rb` 为 `/var`、18:30、免就绪首发。

### 修复后的同一场景

| 时刻 | `save_settings` 次数 | 该次写入里的 `rb` / `rs` | 主文件 |
|---|---|---|---|
| W0 返回、B 已落盘未返回 | 2 | 与事前逐字段相同，`inbound.rules` 整体 `deepEqual` 事前快照 | 规则与事前相同，`fontScale` 1.2 |
| B 返回、C 发出 | 3 | 与事前相同 | |
| C 落盘 | 3 | | 三次写入的 `inbound.rules` 都等于事前快照；`fontScale` 1.2、两个快捷键都是新值；内存中的规则与事前相同 |

C 失败时：主文件收到 2 次写入，规则等于事前快照，`fontScale` 1.2。

### 同一断言修复前后

修复前逐项单独运行（合跑时一项失败会卡住写入队列，使后面的测试超时）；修复后整文件运行。

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `a clock rule save given up before its turn is in no other save's write` | 失败 `B's write does not carry what A wanted: true !== false` | 通过 |
| `a badge rule save given up before its turn is in no other save's write` | 失败，同一断言 | 通过 |
| `with the last queued save failing, a save given up earlier is still nowhere in the file` | 失败，主文件规则 `deepEqual` 事前快照不成立（`/var`、1110、`firstSendWithoutReadiness`） | 通过 |
| `a rule save that goes ahead is kept by the saves queued behind it, and they keep their own` | 通过 | 通过 |
| `a rule save already sent is not taken back, and the save behind it carries it` | 通过 | 通过 |
| `a permission already given is kept, and not asked for again, by a save of another setting` | 通过 | 通过 |
| `a save started after one was given up is written as its own, behind the saves already waiting` | 失败，前三次写入的规则不等于事前快照 | 通过 |
| `a rule save that fails in the file takes back its own rules only, and no later save writes them` | 失败，同上 | 通过 |

合计：新增 8 项修复前 `pass 3 / fail 5`；修复后整文件 `tests 22 / pass 22 / fail 0`。F3.1 的 14 项修复前后都通过。

### 保持性断言

- A 合法继续：恰好 4 次写入（W0、A、B、C）。W0 的写入里 `rb` 是事前内容；A、B、C 三次写入里 `rb` 都是 `/var`、1110、免就绪首发，`rs` 与事前相同；最终主文件 `fontScale` 1.2、两个快捷键为新值；A 的编辑器由它自己的保存关闭。
- A 已发出后打开另一条规则：A 的写入照常完成，随后 B 的写入里 `rb` 仍是 `/var`，`rs` 与事前相同，新打开的编辑器保持可用。
- A 已发出但原生写入失败：主文件收到 W0、B、C 三次写入，规则都等于事前快照；内存中的规则等于事前，字号和快捷键的新值仍在内存；失败提示 1 次；A 的编辑器保持打开。
- 已有频道授权：先签发 `native-1`，再改字号和快捷键。后两次写入没有授权请求，三次写入里 `ra` 的授权 ID 都是 `native-1`。
- A 取消且轮次已过之后的新保存：前三次写入规则等于事前快照；第四次是新保存自己的写入，`rs` 目录为 `/opt`，`rb` 与事前相同，字号和快捷键保留。

### 层级

node 下的测试 DOM。保存按钮处理函数、`saveRule`、`persistInbound` / `commitSettings` / 写入队列、`setFontScale`、`setShortcut` 是生产代码。合成的只有 `window.__TAURI__.core.invoke`：`save_settings` 的“发出”“主文件被替换”“响应返回”由测试分别放行，写入可被拒绝；`load_settings` 返回主文件当前内容。表情规则的批准哈希由真实的 `crypto.subtle` 计算，测试等它算完再排入 B 和 C。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS | 通过 |
| `scripts/ui-tests` | `tests 597 / pass 597 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略（在加入 `automation.js` 头部一句注释和文档之前的工作树上运行；之后重跑了读取前端源码的五个集成测试 `ipc_contract`、`signal_census`、`log_privacy`、`edr_quiet`、`external_admission`，45 项通过） |

没有构建应用，没有启动应用或 tmux 服务器，没有需要清理的进程、socket 或 bundle。fmt、clippy、245 秒后台载体没有重跑。

## F3.3 正常加载基线与首笔保存失败隔离（2026-10-07）

起始 `main` / `fe1cd93` / 工作树干净。生产改动：`app/ui/js/settings.js`，`app/ui/js/automation.js`（头部契约一句）。测试：新增 `app/ui/test/settings-first-save-dom.test.mjs` 和 `app/ui/test/fixtures/first-save-scenes.mjs`（9 项）；`channel-editor-race-dom.test.mjs` 改一条断言和测试名。

### 初始化差异（读代码并 grep 确认）

- `app.js` 的 `boot()` 调用 `loadSettings()`。`fe1cd93` 的 `loadSettings()` 只设置 `ctx.settings`，不碰 `committedInbound` / `nativeChannelSettings`。
- `refreshChannelAuthority()` 的唯一生产调用点是 `channel-authority-changed` 事件监听。
- `channel-editor-race-dom.test.mjs` 的 `scene()` 每次调用 `refreshChannelAuthority()`。

### 修复前的实际写入（`fe1cd93` 的 `settings.js`，独立进程，只经 `loadSettings()`）

主文件：频道规则 `ra`（有授权 `native-0`）、Clock 规则 `rb`（`/tmp`、09:00）、表情规则 `rs`（`/tmp`、无 `autoSend`）。顺序：A 编辑器保存（本次运行第一笔 `save_settings`，已发出、扣住）→ B `setFontScale(1.2)` → C `setShortcut` → A 被拒绝且主文件未写入 → B 落盘、扣住响应 → 放行 B、C → 再做一次 `setFontScale(1.3)`。

A 为 `rb`（目录 `/var`、18:30、免就绪首发）：

| 时刻 | 发出的保存 | 主文件被写次数 | 该次请求里的 `rb` | 主文件里的 `rb` |
|---|---|---|---|---|
| A 被拒绝，B 已落盘未答复 | 2 | 1 | `dir:/var, minute:1110, firstSendWithoutReadiness:true` | 同左 |
| C 落盘 | 3 | 2 | `dir:/var, minute:1110, firstSendWithoutReadiness:true` | 同左 |
| 之后再保存一次字号 | 4 | 3 | 同上 | 同上 |

内存里的 `rb` 同样是 `/var`、1110、免就绪首发。A 为 `rs`（目录 `/var`、勾选自动发送）时，B、C、之后那次保存的请求和主文件、以及内存里，`rs` 都是 `dir:/var` 并带 `autoSend`（键 `classes, digest, external, steps`）。两种情况授权请求列表为空。

### 修复后的同一场景

| 时刻 | 发出的保存 | 主文件被写次数 | 该次请求的自动化配置 | 主文件 |
|---|---|---|---|---|
| A 被拒绝，B 已落盘未答复 | 2 | 1 | 整个 `inbound` `deepEqual` 加载时的内容 | `inbound` 等于加载时的内容，`fontScale` 1.2 |
| C 落盘 | 3 | 2 | 同上 | 同上，快捷键为新值 |
| 之后再保存一次字号 | 4 | 3 | 同上 | |

内存：规则等于加载时的内容，字号和快捷键是新值；被拒绝的保存的编辑器保持打开。C 也被拒绝的场景：主文件只被写 1 次（B），`inbound` 等于加载时的内容，`fontScale` 1.2。

### 同一断言修复前后

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `a clock rule save refused as the first save of a run is in no later save's write` | 失败 `B's write does not carry what the refused save wanted: true !== false` | 通过 |
| `a badge rule approval refused as the first save of a run is in no later save's write` | 失败，同一断言 | 通过 |
| `with the last queued save failing too, the refused first save is nowhere in the file` | 失败，主文件 `inbound` 不等于加载时的内容（`/var`、1110、`firstSendWithoutReadiness`） | 通过 |
| 其余 6 项（首笔成功、提交前取消、授权保持、备份、读取失败、首次运行） | 通过 | 通过 |

合计：修复前 `pass 6 / fail 3`；修复后 `tests 9 / pass 9 / fail 0`。

### 保持性断言

- 首笔保存成功：主文件被写 3 次（A、B、C），三次里 `rb` 都是 `/var`、1110、免就绪首发，`rs` 与加载时相同，`ra` 的授权 ID 都是 `native-0`；最终 `fontScale` 1.2、快捷键为新值；A 的编辑器关闭。
- 提交前取消：W0（快捷键）已发出，A 排队，B、C 排队，取消 A，W0 被拒绝（本次运行没有任何成功的保存）。发出 3 次保存（W0、B、C），主文件被写 2 次，两次的 `inbound` 都等于加载时的内容。
- 授权保持：加载后改字号、改快捷键，两次写入的 `inbound` 等于加载时的内容，`ra` 的 `firstSend` 与授权原样，授权请求为空。
- 备份：加载后主文件字节不变；内存里 `ra` 没有 `firstSend` 和授权，其余规则可用；字号保存发出（所有者的保存可用），请求里 `ra` 没有频道选择和授权、没有授权请求；随后一笔被拒绝的规则保存不出现在下一次写入里；再从主文件加载一次并保存，`ra` 仍然没有授权。
- 读取失败：没有任何 `save_settings`，有失败提示，内存里是默认值。
- 首次运行：没有提示，第一次字号保存一次完成，写入的自动化配置为空。

### 层级

每个场景是一个独立的 node 进程（`node fixtures/first-save-scenes.mjs <场景>`，由测试文件 `spawnSync` 启动，退出码 0 为通过）。生产代码：`loadSettings`、设置写入队列、`commitSettings`、编辑器保存按钮与 `saveRule`、`setFontScale`、`setShortcut`。合成：`window.__TAURI__.core.invoke`，其中 `load_settings` 按场景回答主文件、备份（去掉频道选择与授权，模拟原生的交出方式）、首次运行或失败。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS 加两个新文件 | 通过 |
| `scripts/ui-tests` | `tests 606 / pass 606 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略（在最终的前端源码上运行） |

没有构建应用，没有启动应用或 tmux 服务器。fmt、clippy、245 秒后台载体没有重跑。

## F3.4 普通设置失败对已提交规则的回滚隔离（2026-10-07）

起始 `main` / `bc3adc0` / 工作树干净。生产改动：`app/ui/js/settings.js`（失败分支一处，加契约注释）。测试：`app/ui/test/settings-first-save-dom.test.mjs` 与 `fixtures/first-save-scenes.mjs` 新增 6 个场景。

### 修复前的逐阶段状态（`bc3adc0` 的 `settings.js`，独立进程，只经 `loadSettings()`）

主文件 S0：频道规则 `ra`（`firstSend`，授权 `native-0`）、Clock 规则 `rb`（`/tmp`、09:00）、表情规则 `rs`（`/tmp`、无 `autoSend`）。顺序：A 编辑器保存（已落盘，扣住答复）→ B `setFontScale(1.2)` 排队 → A 答复成功 → B 发出并被拒绝（主文件未写入）→ D 在另一条规则的编辑器里把目录改为 `/opt` 并保存。

A 为 `rb`（目录 `/var`、18:30、免就绪首发）：

| 阶段 | 内存里的 `rb` | 主文件里的 `rb` |
|---|---|---|
| A 答复后 | `/var`、1110、免就绪首发 | 同左 |
| B 被拒绝后 | `/tmp`、540、无 | `/var`、1110、免就绪首发 |
| D 的请求 | `/tmp`、540、无（`rs` 为 `/opt`） | |
| D 落盘后 | | `/tmp`、540、无 |

A 为 `rs`（目录 `/var`、批准自动发送）：A 答复后内存与主文件里 `rs` 为 `/var` 且有 `autoSend`；B 被拒绝后内存里 `rs` 为 `/tmp`、无 `autoSend`，主文件不变；随后 D 的保存处理函数抛出 `TypeError: Cannot read properties of undefined (reading 'external')`（见报告“另外发现”）。

A 为 `ra`（撤下首步授权）：

| 阶段 | 内存里的 `ra` | 主文件里的 `ra` |
|---|---|---|
| A 答复后 | 无 `firstSend`、无授权 | 同左 |
| B 被拒绝后 | `firstSend: true`、授权 `native-0` | 无 `firstSend`、无授权 |
| D 的请求 | `firstSend: true`、授权 `native-0`（`rb` 为 `/opt`） | |
| D 落盘后（合成原生侧照单写入） | | `firstSend: true`、授权 `native-0` |

三种情况授权请求列表都为空。

### 修复后

同样的三个场景，每一阶段内存里的自动化配置都 `deepEqual` 主文件：B 被拒绝后内存保留 A；D 的请求、落盘后的主文件、D 之后的内存三处都保留 A（`rb` 为 `/var`、1110、免就绪首发；`rs` 为 `/var` 且有 `autoSend`；`ra` 无 `firstSend`、无授权），D 自己的目录修改为 `/opt`。B 自己的字段（字号 1，或快捷键 `Meta+KeyD`）已收回，失败提示 1 次，主文件写入次数在 B 前后不变。

### 同一断言修复前后

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `a clock rule save that landed is kept in memory and in the next rule save when a font save fails` | 失败 `the automations in memory are still the ones in the file` | 通过 |
| `a clock rule save that landed is kept when a shortcut save fails` | 失败，同一断言 | 通过 |
| `a badge approval that landed is kept in memory and in the next rule save when a font save fails` | 失败，同一断言 | 通过 |
| `a channel permission withdrawn by a save that landed is not brought back by a failed font save` | 失败，同一断言 | 通过 |
| `a rule save and a font save that both landed are both kept by the next rule save` | 通过 | 通过 |
| `a font save that fails alone takes back its own change and leaves every automation and permission` | 通过 | 通过 |

整个文件：修复前 `tests 15 / pass 11 / fail 4`；修复后 `tests 15 / pass 15 / fail 0`。

### 保持性断言

- A、B 都成功：主文件写入 2 次后 D 再写 1 次；D 的请求、主文件、内存里 `rb` 都是 A 的版本，`rs` 为 `/opt`，`ra` 的授权仍是 `native-0`，最终主文件 `fontScale` 1.2。
- B 单独失败：提示 1 次，内存 `fontScale` 回到 1，主文件未写；随后 D 的请求和主文件里 `ra` 的 `firstSend` 与授权原样、`rb` 原样、授权请求为空；同一个字号设置再保存一次成功。

### 既有异常的核对

在隔离目录建 `66f8874` 的 git worktree，放入当前的场景文件并加一个只做“加载 → 撤销 `rs` 的批准并保存”的临时场景，运行输出 `TypeError: Cannot read properties of undefined (reading 'external')`。worktree 已用 `git worktree remove` 移除；临时场景没有进入仓库。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS | 通过 |
| `scripts/ui-tests` | `tests 612 / pass 612 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略（在最终的前端源码上运行） |

没有构建应用，没有启动应用或 tmux 服务器。fmt、clippy、245 秒后台载体没有重跑。

## R1 表情规则撤销批准后的界面一致性（2026-10-08）

起始 `main` / `9acc59e` / 工作树干净。生产改动：`app/ui/js/automation.js`（批准缓存的读取与发布、头部契约），`app/ui/js/automation-model.js`（`approvalText`）。测试：新增 `app/ui/test/automation-approval-dom.test.mjs`（7 项）；`fixtures/first-save-scenes.mjs` 与 `settings-first-save-dom.test.mjs` 各加 1 个场景。

### 读取点（修复前）

| 位置 | 行为 |
|---|---|
| `automation.js` `approvals` | `Map<规则 ID, 状态>`，由 `refreshApprovals` 整体替换 |
| `renderAutomations` | 先同步 `paintAutomations()`，再异步 `refreshApprovals()`；结果返回后只有“是否重绘”受 `renderSeq` 限制，缓存无条件替换 |
| `ruleEl` → `ruleFacts` → `approvalText(rule, state)` | `state === 'valid'` 时读 `rule.autoSend.external` |
| `openEditor` | `approvals.get(rule.id) === 'valid'` 时勾选并读 `rule.autoSend.external` |
| `saveRule` | `persistInbound` 成功后同步调用 `renderAutomations()`，异常向上抛到保存按钮处理函数 |

### 同一断言修复前后（修复前逐项单独运行）

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `taking a badge rule's approval away is one save: the list, the editor and the file agree` | 失败 `TypeError: Cannot read properties of undefined (reading 'external')` | 通过 |
| `an approval computed earlier is not shown or ticked for a rule that no longer has one` | 失败，同一 `TypeError` | 通过 |
| `an approval result that arrives late, for the rule as it was, changes nothing` | 失败，同一 `TypeError`（发生在撤销保存处，未到迟到结果） | 通过 |
| `a result that arrives late, for the rule before it was approved, does not hide the approval` | 失败 `AssertionError: the editor still opens approved: saving it would not take the approval away` | 通过 |
| `approved, approved with message content, changed since, and never approved read as before` | 失败 `AssertionError: not called approved before it was checked against the new steps` | 通过 |
| `a refused save of the same change leaves the approval that is in the file, and says the save failed` | 通过 | 通过 |
| `after taking one approval away, saving another rule and approving again all work once each` | 失败，同一 `TypeError` | 通过 |
| `a badge approval taken away by a save that landed is not brought back by a failed font save`（F3.4 场景） | 失败，同一 `TypeError` | 通过 |

合计：新文件修复前 `pass 1 / fail 6`，修复后 `tests 7 / pass 7 / fail 0`；补回的场景修复前失败、修复后通过。

### 各测试的受控时序

- **缓存滞后**：列表里已批准的 `ra` 排在 `rs` 前面。扣住所有哈希后撤销 `rs` 并保存：保存后的重新计算停在 `ra` 的哈希上，`rs` 的缓存仍是撤销前的 `valid`。此时断言 `rs` 一行显示关闭、编辑器关闭、重新打开两个框都不勾；放行后 `rs` 仍是关闭，`ra` 显示开启。
- **迟到的旧 valid**：扣住哈希，触发一次重绘（开始计算已批准的 `rs`），撤销并保存（新一轮计算对 `rs` 不需要哈希，立即完成），再放行旧计算。之后列表、编辑器、再一次重绘都不显示开启。
- **迟到的旧“未批准”**：`ra` 已批准、`rs` 未批准。扣住哈希并触发重绘（旧计算停在 `ra`），解除扣留后批准 `rs` 并保存，等列表显示开启，再放行旧计算。之后打开 `rs` 批准框仍勾选，再一次重绘仍显示开启；保存共 1 次。
- **保持性**：五条规则依次为固定模板已批准、带消息内容的模板连同外部内容一起批准、批准后规则目录被改、批准后模板不存在、从未批准。列表文案依次为已开启、已开启含消息内容、规则已改、模板已改、关闭；编辑器勾选依次为 `[是,否] [是,是] [否,否] [否,否] [否,否]`。随后改动固定模板的步骤并重绘：计算完成前不显示已开启，完成后显示模板已改，编辑器不勾；全程没有保存。

### 补回的 F3.4 场景（独立进程，只经 `loadSettings()`）

主文件里 `rs` 带有由 `approveRule` 生成的批准。A 在编辑器里取消批准并保存（已落盘、扣住答复）→ B `setFontScale(1.2)` 排队 → A 答复 → B 被拒绝且主文件未写 → D 把 `rb` 的目录改为 `/opt` 并保存。

| 阶段 | `rs.autoSend` |
|---|---|
| A 答复后，内存与主文件 | 无 |
| B 被拒绝后，内存（与主文件 `deepEqual`） | 无；打开编辑器批准框不勾 |
| D 的请求 | 无 |
| D 落盘后的主文件 | 无 |
| D 之后的内存 | 无；打开编辑器批准框不勾 |

D 自己的目录修改已保存，授权请求为空，B 的失败提示 1 次。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS 加新测试文件 | 通过 |
| `scripts/ui-tests` | `tests 620 / pass 620 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略（在最终的前端源码上运行） |

层级：node 下的测试 DOM，不是真实 WKWebView。没有构建或启动应用，没有 tmux 服务器。fmt、clippy、245 秒后台载体没有重跑。

## R1.1 待验证批准与编辑器保存意图隔离（2026-10-08）

起始 `main` / `97ed5bc` / 工作树干净。生产改动：`app/ui/js/automation.js`、`app/ui/js/automation-model.js`、`app/ui/js/i18n/en.js`、`app/ui/js/i18n/zh-Hans.js`（各 3 条文案）、`app/ui/index.html`（提示行 `auto-send-check`）。测试：`app/ui/test/automation-approval-dom.test.mjs` 新增 7 项，`scene()` 增加不等待哈希的冷启动选项。

### 修复前的实际结果（`97ed5bc`，逐项单独运行）

场景：主文件里 `rs` 带 `approveRule(rule, 固定模板)` 生成的批准；打开抽屉前扣住哈希；点列表上的编辑按钮；把名称改为 `Renamed`；放行哈希；按一次保存。

| 读取点 | 结果 |
|---|---|
| 保存请求里的 `rs.autoSend` | 无（断言 `the request keeps the approval as it was` 失败） |
| 先按保存再放行哈希的变体 | 同样丢失（断言 `the approval is kept` 失败） |
| 带外部内容确认的批准（消息模板，`external: true`） | 同样丢失 |
| 哈希失败时的列表 | 没有“无法核对”的状态 |

### 同一断言修复前后

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `an approved rule opened before its approval was checked keeps the approval through an ordinary save` | 失败 `the request keeps the approval as it was` | 通过 |
| `Save pressed before an approval was checked waits for the check and is one save that keeps it` | 失败 `the approval is kept` | 通过 |
| `Save pressed before an approval that includes message content was checked waits for the check and is one save that keeps it` | 失败 `the approval is kept` | 通过 |
| `unticking while the approval is being checked is the user's decision: a late valid does not tick it again` | 通过 | 通过 |
| `what the approval covers changing during the check leaves the new version unapproved` | 通过 | 通过 |
| `a check that answers late touches no other editor` | 通过 | 通过 |
| `an approval that cannot be checked is neither saved away nor called on` | 失败 `the list says it could not be checked` | 通过 |

合计：新增 7 项修复前 `pass 3 / fail 4`；修复后整文件 `tests 14 / pass 14 / fail 0`。

### 修复后的逐场景读取

- **核对完成后保存**：核对前列表为“正在核对已保存的批准…”；编辑器 `{勾选: 是, 外部: 否, 待定: 是, 提示: 正在核对…}`；放行后 `{是, 否, 否, 无}`；保存 1 次，请求、主文件、内存的 `autoSend` 与原批准 `deepEqual`，名称为 `Renamed`，列表显示开启，“已保存”1 次。
- **保存先于核对**：按下保存后、放行前 `save_settings` 0 次；放行后共 1 次，`autoSend` 与原批准 `deepEqual`（外部批准的场景里包含 `external: true`），编辑器关闭，“已保存”1 次。
- **明确撤销**：待定时取消勾选，编辑器立即 `{否, 否, 否, 无}`；放行（结果为 valid）后不变；保存 1 次，请求和主文件无 `autoSend`，列表显示关闭。
- **语义变化**：
  - 待定时改目录：勾选被既有规则取消，放行后仍不勾，保存后目录为 `/var`、无 `autoSend`。
  - 待定时模板步骤被改：放行后不勾，保存后无 `autoSend`。
  - 批准一开始就过期：放行后不勾，列表显示“规则已改”。
- **切换与取消**：打开已批准的 `ra`（待定）后立即打开从未批准的 `rn`：编辑器为 `{否, 否, 否, 无}`，放行后不变；保存 `rn` 1 次，`rn` 无 `autoSend`，`ra` 的批准原样。再让所有批准需要重新核对，打开 `ra` 后取消，放行：编辑器保持关闭，保存按钮未被锁住。
- **核对失败**：列表显示“无法核对已保存的批准”；编辑器 `{是, 否, 是, 无法核对…}`；改名后保存：`save_settings` 0 次，没有“已保存”，失败说明 1 次，编辑器保留，主文件批准原样。取消并恢复哈希后：列表显示开启，编辑器 `{是, 否, 否, 无}`。再次令哈希失败并使批准需重新核对：打开、明确取消勾选、保存，写入 1 次，主文件无 `autoSend`。

全部场景结束时没有未处理的 rejection。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS | 通过 |
| `scripts/ui-tests` | `tests 627 / pass 627 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略（在最终的前端源码上运行） |

层级：node 下的测试 DOM，不是真实 WKWebView；复选框的半选状态在测试里是一个被读写的属性。没有构建或启动应用，没有 tmux 服务器。fmt、clippy、245 秒后台载体没有重跑。

## R1.2 批准版本绑定与真实控件收尾（2026-10-08）

起始 `main` / `6b15ab8` / 工作树干净。

改动范围：

- 生产前端：`app/ui/js/automation.js`（勾选来历、步骤漂移撤下、保存意图、头部契约），`app/ui/index.html`（提示行移出触发类型显隐组），`app/ui/js/i18n/en.js`、`zh-Hans.js`（各 1 条文案）。
- 调试冒烟：`app/ui/test/approval-smoke.mjs`（新），`app/ui/test/wk-smoke.mjs`（入口），`app/ui/test/fixtures/smoke-manifest.json`，`app/ui/test/smoke-manifest.test.mjs`，`app/src-tauri/src/main.rs`（模式表一行），`app/src-tauri/src/diagnostics.rs`（检查点词表九行），`app/run.sh`（模式列表），`app/SMOKE.md`。
- Node 测试：`app/ui/test/automation-approval-dom.test.mjs` 新增 7 项（文件共 21 项）。
- 证据：`docs/evidence/approval-control-r1.2/` 六张截图（裁到编辑器面板，缩到半分辨率）。

### A. 模板漂移

修复前（`6b15ab8` 的前端，逐项单独运行）与修复后：

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `a template changed while the editor is open is not approved by an ordinary save (approval already checked)` | 失败 `the request carries no approval of the new steps` | 通过 |
| `a template changed while the editor is open is not approved by an ordinary save (approval checked after the editor opened)` | 失败，同一断言 | 通过 |
| `an ordinary save keeps the saved approval as it is, with the approval already checked` | 通过 | 通过 |
| `ticking the box again approves the steps as they are then, plain or with message content` | 失败（编辑器状态断言：修复前模板变化后仍勾选且无提示） | 通过 |
| `steps changed after the user approved, while that save is still hashing, are not what gets approved` | 通过 | 通过 |
| `a template change touches no decision the user made and no other editor` | 通过 | 通过 |
| `changing what happens when the run ends withdraws the tick, from the real buttons`（C 项） | 通过 | 通过 |

修复后的读取：

- **热缓存 / 冷缓存，模板改为 M1 后普通保存**：模板保存后编辑器为 `{勾选: 否, 外部: 否, 待定: 否, 提示: 模板在批准后已修改…}`；保存 1 次；请求里该规则没有 `autoSend`，`grantDetail(请求规则, M1)` 不是 `valid`；名称已保存；列表显示关闭；“已保存”1 次。
- **展示字段**：带外部内容确认的批准，热缓存打开为 `{是, 是, 否, 无}`，只改名称后保存 1 次，请求里的 `autoSend` 与原批准 `deepEqual`，列表显示“已开启（含消息内容）”。
- **明确重新批准**：模板改为 M1 后勾选批准框，保存 1 次；请求里的批准对 M1 为 `valid`、对 M0 为 `stale-template`，`external` 为 `false`。带消息内容的模板改动后编辑器先被撤下勾选并显示提示，勾选批准框和外部内容框后保存，批准对新步骤为 `valid`，`external` 为 `true`。
- **批准 M1 后哈希期间变为 M2**：保存 1 次；请求里的批准对 M2 不是 `valid`、对 M1 为 `valid`；列表显示“模板已改”。
- **不触碰用户的决定和其他编辑器**：明确取消勾选后模板变化，编辑器保持 `{否, 否, 否, 无}`，不出现提示；打开从未批准的规则后模板变化，同样无变化；没有编辑器打开时模板变化，没有保存，没有未处理的 rejection。

模板变化在测试里都通过 `provider.saveTemplate`（模板管理器用的同一个入口：一次 Board 事务，然后发出 `projects`）。

### B. 真实 WKWebView

| 项目 | 值 |
|---|---|
| 源码 | `6b15ab8` 加本节的工作树改动（运行时尚未提交；运行之后只改了文档） |
| 载体二进制 | 调试构建 `target/debug/deck-app`，版本 0.7.24，ad-hoc 签名，bundle id `io.c9r.deck.smoke`。第七次运行（截图来自这次）SHA-256 `2390951b5e4b5c2487bd8f6aa0609ed739df870122a52cbbb76d535e2bf9816f`；门禁之后重新构建的最终二进制 SHA-256 `d56ce985006b8826e69139163bb21130677cdaf13a32e56e19f7c8d3ea19c5ec`，第八次运行用它，前端源码与第七次相同 |
| 机器 | Mac mini 测试机，macOS 27.0.1 |
| 隔离 | 第七次：数据目录 `/private/tmp/deck-verify-v641fa47f/approval/data`，socket `deck-smoke-v641fa47f-approval`；第八次：`/private/tmp/deck-verify-v895c5464/approval/data`，`deck-smoke-v895c5464-approval`。都作为启动参数传入；20 秒内确认隔离目录的 `app.log` 出现 |
| 模式 | `--smoke-wkwebview approval` |
| 页面尺寸 | 1280×768（窗口 1280×800） |

`app.log` 中的检查点（第七次与第八次逐行相同）：

```
smoke-check approval-pending a=1 b=1
smoke-check approval-press-box a=1 b=1
smoke-check approval-press-label a=1 b=0
smoke-check approval-key-space a=1 b=1
smoke-check approval-save-through a=1 b=1
smoke-check approval-check-failed a=1 b=1
smoke-check approval-layout a=1280 b=768
smoke-check approval-drift a=1 b=1
smoke-check done a=1 b=0
```

`scripts/smoke-verdict`：`PASS (9/9 expected checkpoints, 9 lines)`；`js-error` / `js-reject` 0 行。保存类检查点的 `b` 是该步骤发出的 `save_settings` 请求数。

截图（`docs/evidence/approval-control-r1.2/`）：

| 文件 | 内容 |
|---|---|
| `approval-pending-en.png` | 英文，待核对：原生半选框，下方“checking the saved approval…” |
| `approval-checked-en.png` | 同一编辑器核对完成：正常勾选，提示行消失 |
| `approval-unticked.png` | 从待定按一次复选框之后，迟到的 valid 返回之后：未勾选 |
| `approval-pending-zh.png` | 中文，待核对 |
| `approval-failed-zh.png` | 中文，核对失败：半选框，下方“无法核对已保存的批准” |
| `approval-template-changed.png` | 已核对的勾选在模板保存后被撤下，下方显示模板已修改的提示 |

未通过的六次运行（脚本逐次修正，原始日志在隔离目录，未入库）：

| 次 | 停在 | 原因 |
|---|---|---|
| 1 | `approval-save-through`、失败场景 | 冒烟脚本：用键顺序敏感的方式比较批准（文件返回的键是排序过的）；失败提示的 toast 盖住了保存按钮，第二次按下落在 toast 上 |
| 2、3 | `approval-save-through` | 冒烟脚本：名称输入框有焦点时 WebKit 把视图滚回去，保存按钮不在按下的位置 |
| 4、5、6 | `approval-drift` | 产品：提示行被“按触发类型显隐”的逻辑重新显示（见报告 B），热缓存打开时读到“勾选且提示行可见” |

清理：每次运行结束后先停应用再停它的 tmux 服务器，`pgrep -lf <运行根目录>` 与 socket 目录检查为空后，把运行根目录移到测试机的废纸篓。八次运行的 `leftover` 都为空；全部结束后另从测试机查询，没有来自这些运行根目录的进程。测试机自己的 `~/.deck` 在运行前后的时间戳相同（`Oct  5 17:59`），它上面安装的正式版 Deck 进程（PID 10546）前后都在，没有被触碰。

### C. `finish`

`app/ui/js/automation.js` 中 `auto-finish` 按钮的处理：

```
if (segGet('auto-finish') !== b.dataset.v) withdrawApproval();
segSet('auto-finish', b.dataset.v);
```

`grep` 结果：`segSet('auto-finish', …)` 只出现在这里和 `openEditor` 的初始化里；`finish` 的读取只有 `readEditor` 的 `segGet('auto-finish')`。测试通过 `initAutomation` 接线后的按钮 `onclick` 触发，在修复前后的代码上都通过。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS 加新冒烟模块 | 通过 |
| `scripts/ui-tests` | `tests 634 / pass 634 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo fmt --check` | 退出码 0 |
| `cargo clippy --workspace -- -D warnings` | 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略；最终工作树上重跑 `smoke_` 相关 9 项与五个读取前端源码的集成测试 45 项，通过 |

## U1 批准提示与首发复选框局部布局收尾（2026-10-08）

起始 `main` / `6d5af6d` / 工作树干净。

改动范围：

- 生产：`app/ui/style.css`（`#auto-editor input` 排除复选框；`#auto-send-check` 两条规则）。
- 调试冒烟：`app/ui/test/approval-smoke.mjs`（三个新检查点、模板与字号参数），`app/ui/test/fixtures/smoke-manifest.json`，`app/src-tauri/src/diagnostics.rs`（词表三行），`app/SMOKE.md`。
- 证据：`docs/evidence/approval-layout-u1/` 九张截图（裁到编辑器面板，半分辨率）。

### 修复前的测量（真实 WKWebView，未修复样式，默认字号，一次性诊断运行）

| 量 | `auto-send` 英 | `auto-first-send` 英 | `auto-review` 英 | `auto-send` 中 | `auto-first-send` 中 | `auto-review` 中 |
|---|---|---|---|---|---|---|
| `flex-grow` / `flex-shrink` / `flex-basis` | 1 / 1 / 0 | 1 / 1 / 0 | 1 / 1 / 0 | 1 / 1 / 0 | 1 / 1 / 0 | 1 / 1 / 0 |
| 计算宽度 px | 80.8 | 0 | 96.2 | 214.1 | 174.1 | 130.8 |
| 计算高度 px | 12 | 12 | 12 | 12 | 12 | 12 |
| `min-width` | 0 | 0 | 0 | 0 | 0 | 0 |
| `display` 非 none / `visibility` visible / `opacity` | 是 / 是 / 1 | 是 / 是 / 1 | 是 / 是 / 1 | 是 / 是 / 1 | 是 / 是 / 1 | 是 / 是 / 1 |
| 矩形宽 × 高 px | 80.8 × 12 | 0 × 12 | 96.2 × 12 | 214.1 × 12 | 174.1 × 12 | 130.8 × 12 |
| label 宽 px | 365 | 365 | 365 | 365 | 365 | 365 |
| 文字 span 宽 × 高 px | 276.2 × 15 | 357 × 30 | 260.8 × 15 | 142.9 × 15 | 182.9 × 15 | 226.2 × 15 |
| label `scrollWidth` / `clientWidth` | 365 / 365 | 365 / 365 | 365 / 365 | 365 / 365 | 365 / 365 | 365 / 365 |
| label `overflow` 为 visible | 是 | 是 | 是 | 是 | 是 | 是 |
| 框左缘距 label 左缘 px | 0 | 0 | 0 | 0 | 0 | 0 |
| 文字左缘距 label 左缘 px | 88.8 | 8 | 104.2 | 222.1 | 182.1 | 138.8 |
| 滚动到位后中心点命中该控件 | 是 | 否 | 是 | 是 | 是 | 是 |

英文首发那一行：文字折成两行（高 30），占满 357 像素，复选框宽 0。其他行里复选框被拉宽到剩余空间，文字因此靠右。

### 检查点（`app.log` 原样）

未修复样式（第 1 次运行）的新增检查点：

```
smoke-check approval-boxes a=-16 b=0
smoke-check approval-note-apart a=-1 b=0
smoke-check approval-exception a=-11 b=0
```

`approval-boxes`：四种组合无一通过，最窄的框 0 像素。`approval-note-apart`：间距 0，字重不更重。首发交互在第 11 阶段超时（英文默认字号下那个框宽 0，无法按到），所以 `approval-first-send` 没有产生。

最终二进制（第 5 次运行）：

```
smoke-check approval-pending a=1 b=1
smoke-check approval-press-box a=1 b=1
smoke-check approval-press-label a=1 b=0
smoke-check approval-key-space a=1 b=1
smoke-check approval-save-through a=1 b=1
smoke-check approval-check-failed a=1 b=1
smoke-check approval-layout a=1280 b=768
smoke-check approval-drift a=1 b=1
smoke-check approval-boxes a=31 b=12
smoke-check approval-note-apart a=9 b=1
smoke-check approval-first-send a=7 b=0
smoke-check done a=1 b=0
```

`approval-boxes a=31`：16 加四个组合各一位（英文 100%、英文 160%、中文 100%、中文 160%），全部成立；`b=12` 是所有被量的复选框里最窄的宽度。`approval-note-apart a=9`：最小间距 8 像素加 1；`b=1` 字重更重。`approval-first-send a=7`：七步都成立；`b=0` 结束时未勾选。`scripts/smoke-verdict`：`PASS (12/12 expected checkpoints, 12 lines)`，`js-error` / `js-reject` 0 行。

### 载体与运行

| 项目 | 值 |
|---|---|
| 源码 | `6d5af6d` 加本节的工作树改动（运行时尚未提交；最终运行之后只改了文档和 `SMOKE.md`） |
| 最终载体二进制 | 调试构建 `target/debug/deck-app`，SHA-256 `d0152ce4695ccfc3d18f9f9326c0b91a271cebdfc20a3efa75c6b5f2fa0e4e13`，版本 0.7.24，ad-hoc 签名，bundle id `io.c9r.deck.smoke` |
| 机器 | Mac mini 测试机，macOS 27.0.1 |
| 最终运行的隔离 | 数据目录 `/private/tmp/deck-verify-vbce5b953/approval/data`，socket `deck-smoke-vbce5b953-approval`，作为启动参数传入；20 秒内确认隔离目录的 `app.log` 出现 |
| 模式 | `--smoke-wkwebview approval`，用时 44 秒 |
| 启动时的机器状态 | 未锁屏；HID 空闲 134 秒；前台是别的应用。测试窗口被带到前台 |

### 截图（`docs/evidence/approval-layout-u1/`）

| 文件 | 内容 |
|---|---|
| `before-note-en.png` | 修复前，英文默认字号，待核对：提示行与说明连在一起 |
| `after-note-en.png` | 修复后，同条件：提示行加粗，下方有间距；复选框文字靠左 |
| `after-note-zh.png` | 修复后，中文，待核对 |
| `after-note-gone-en.png` | 修复后，核对完成：提示行隐藏，不留空行 |
| `before-boxes-en-100.png` | 修复前，英文默认字号：外部内容确认和首发两行没有复选框 |
| `after-boxes-en-100.png` | 修复后，同条件：四个复选框都在 |
| `after-boxes-en-160.png` | 修复后，英文 160% |
| `after-boxes-zh-100.png` | 修复后，中文默认字号 |
| `after-boxes-zh-160.png` | 修复后，中文 160% |

`before-*` 来自第 1 次运行，`after-*` 来自第 5 次运行。截图是 Deck 自己的 WKWebView 内容，不含其他应用。

### 清理

每次运行的 `leftover` 为空。全部结束后从测试机查询：`pgrep -lf deck-verify-` 无结果；`/private/tmp` 下 `deck-verify-` 目录 0 个；tmux socket 目录只有 `deck`；没有 `caffeinate -d -u` 进程；正式版 Deck 进程 PID 10546 仍在；`~/.deck` 时间戳前后相同（`Oct  5 17:59`）。

### 门禁

| 门禁 | 结果 |
|---|---|
| `node --check` 全部已跟踪 UI JS/MJS | 通过 |
| `scripts/ui-tests` | `tests 634 / pass 634 / fail 0`，退出码 0 |
| `node ui/js/check.mjs` | `ok: 58 modules` |
| `git diff --check` | 退出码 0 |
| `cargo fmt --check` / `cargo clippy --workspace -- -D warnings` | 退出码 0 / 退出码 0 |
| `cargo test --workspace` | 1,189 项通过，0 失败，2 项既有忽略 |
