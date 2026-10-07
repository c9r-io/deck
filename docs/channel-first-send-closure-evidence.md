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
