# 频道首步授权：本地验收记录

总体验收：**PASS**，下列 18 组均通过；真实服务验证明确不在本次范围内。

日期：2026-10-07。起始分支 `main`，起始 HEAD `03f9f4ae8121508695f1442dc7da760d6b47d8f0`，起始工作树干净。本记录只涵盖本地合成事件与隔离 fixture；未连接真实 Slack、未启动真实 Claude/Codex、未发布或推送。

## 实现边界

规则仍只有一个首步开关，开启和语义更新与保存合并。原生 settings 生成随机 grant ID，摘要绑定来源身份、范围、分组窗口、项目、冻结目录、完整命令和有效首步配方。原生成功保存之后写入 inbox activation，以微秒边界判断消息资格；ID 不依赖时间唯一性。入队端采用整秒观测区间的保守上界，因此 15 分钟最后一个不完整秒不升级资格。已经成功入队的相同 operation 不重新计龄。

`channel_first_send` 是不可丢失的队列来源约束，`authorized` 是可撤销的当前资格。它与输入就绪分开：已有交互证据也不能绕过频道授权。频道首步不使用 badge/PhoneTasks 的证明，不扩展 `StepAuthority` 或 `TriggerClass`；所有频道行仍为 external，后续 chain 保持手动发送。文本编辑增加 revision 并永久撤下该行自动资格，改回文本不会复活。

首次准入联合验证原生 pending、暂存 grant、当前主 settings、当前可信连接身份、原生卡片绑定及权威 Board 冻结计划。首步 operation 由事件/规则身份确定；精确 operation 重放先于 pending、新鲜度和目录检查。缺失证明、变造目标/文本/序号和换 operation 不会转入 legacy。模板展开使用共享向量，重试使用冻结骨架和目标，不读取当前模板重拼首步。

撤销与最终发送采用 `settings fence → Board fence → queue lock → storage save lock`。firing intent 同时记录本次尝试的自动/手动及 readiness override 依赖；这两项在不确定恢复中也保留，不冒充成功。firing intent 持久化是不可逆边界；在它之前提交的撤销阻止自动投递，之后的一次投递可能继续。兼容等待与实际粘贴不占这些 fence；等待不是就绪证据。未知来源保留意图重试，明确撤销停止自动发送，可能发生粘贴的失败保持 ambiguous。

Slack transport ACK 仍在原生暂存之后；本地 ACK 在全计划入队并持久化 initialQueued 之后。已收集事件先查应用记录，只补 ACK，不因停止收集、过期或规则编辑变成新运行。删除最后一份 Board 应用证明之前先写原生消费/取消记录，旧 drain 快照不能重建。已有 operation 的证明与队列共同保存，不依赖 ACK 后仍有 pending。

settings、Board、queue 使用粘性 v7；独立 inbox 使用 v2，新读取者兼容 v1 但不补授资格。settings 备份恢复在交给 webview 前持久撤下本功能的开关和 grant。Board/queue 回滚涉及的首步保留不确定性，不能推定从未发送。queue 还持久化恢复边界，覆盖备份早于首次 operation 的空队列：没有可信 operation 的旧暂存事件进入 ambiguous。整秒边界及时间回拨下无法可靠排序的事件保守处理；恢复后明确较新的事件不受该边界影响。模板首步变化先退役 grant 再保存 Board；后续保存失败不恢复旧授权，普通后续步骤编辑不退役 grant。

## 证据位置与方法

本机详细日志：`/tmp/deck-channel-first-send.L4bb8p/evidence/`。Rust 测试使用隔离 HOME/TMPDIR，显式固定现有 CARGO_HOME/RUSTUP_HOME 并设置 `CARGO_NET_OFFLINE=true`；Keychain 的单元测试和 smoke 路径在 OS 调用前阻断。每次 WKWebView 使用新数据目录、私有 HOME/ZDOTDIR/TMPDIR、独立 tmux socket 和独立应用 bundle。

以下矩阵中的 `N` 指 `documents::channel_admission_tests::native_channel_admission_binds_source_scope_frozen_intent_and_retirement`：独立子进程内经过生产 Slack 解析、匹配、暂存及原生授权，使用磁盘 settings/inbox 和权威 Board；不使用网络或真实 Agent。`P` 指 `scheduler::channel_permission_tests`。`J` 指 `app/ui/test/channel-*.test.mjs`。它们与真实 WKWebView 证据分开列出。

| 组 | 结论 | 测试与证据 |
|---|---|---|
| 1 旧来源兼容、不追授 | PASS | P `channel_legacy_rows_have_no_new_permission_or_readiness_override`；J `channel grants survive normal saves but legacy settings gain no authority`；inbox v1 加载测试。 |
| 2 仅新运行首步 | PASS | N 首步与精确后续 chain 对照；P `channel_permission_does_not_lift_needs_input_codex_unavailable_or_followup_hold`；真实 smoke `channel-bg-held`。 |
| 3 伪造与省略绕过 | PASS | N 变造正文、operation、命令、session、mode、序号、group、reviewEach、claim/来源标记，以及错误工作区/频道/发送者；`reviewed_list_rejects_channel_first_send_claim`；external_admission/Signal census。 |
| 4 后续收集 ACK 恢复 | PASS | J `collected event with failed ACK only retries ACK after stop, expiry, rule edit and restart`，`applied channel evidence survives collection stop, expiry and rule regrouping`。 |
| 5 两种交互状态下撤销 | PASS | P `channel_permission_revocation_holds_with_and_without_interaction_or_session`；N 调用真实 settings/Board fence 的四种前后撤销矩阵。 |
| 6 session 缺失不越权启动 | PASS | N 无 session 时给 context prepare/probe 注入 panic，确认主 settings 不可读及已撤销均不调用启动路径；P 上述无 session 分支；`channel_unreadability_preserves_permission_and_recovers_without_reacceptance`；发送前启动路径重新检查当前约束和 revision。 |
| 7 临时故障与永久撤销 | PASS | N 主 settings 暂时缺失、Board 不可验证再恢复；P `channel_text_edit_and_revert_keep_the_constraint_withdrawn_and_revision_changed`、`channel_unreadability_preserves_permission_and_recovers_without_reacceptance`。 |
| 8 代次不复活 | PASS | P `channel_grant_ids_do_not_reuse_a_second_or_a_rolled_back_timestamp`、`channel_new_grant_does_not_reauthorize_an_old_constrained_row`；N 无 tick 的关→开与模板改走→改回。 |
| 9 模板与展开契约 | PASS | N 仅后续步骤不退役、首步编辑退役；P `channel_grant_semantics_ignore_collection_order_columns_names_and_later_steps`、`channel_bounded_head_requires_external_acceptance_and_empty_head_is_not_skipped`；共享 `channel-first-send.json` 两端向量；J 暂存后改模板仍用冻结骨架。 |
| 10 生效与 15 分钟 | PASS | `slack_message_microseconds_form_a_strict_grant_boundary`；N 900/901 秒拒绝及旧代次 pending；`completed_channel_operation_replays_before_missing_directory_normalization` 与 operation 幂等测试。 |
| 11 部分入队与 ACK 重放 | PASS | J `channel ACK follows full enqueue and durable initialQueued, including exact-card recovery`、`an ACK response failure after complete enqueue does not enqueue again`、`frozen channel retry ignores observed cwd, current template and current rule option`；N ACK 两次成功、未知 ACK 拒绝、ACK 后不能新造首步。 |
| 12 删除/取消与旧快照 | PASS | `canceled_applied_event_cannot_be_reintroduced_by_a_stale_drain`；J `a canceled event in an already-pulled drain snapshot does not create a run`；原生 Board 证据消费先于删除。 |
| 13 备份回滚 | PASS | `restored_settings_backup_withdraws_channel_first_send_intent_and_grant`；`restored_queue_backup_marks_constrained_channel_rows_ambiguous`（含空备份恢复边界的启动持久化）；N 比较首次 operation 缺失时的暂存边界；N recovered Board 拒绝准入且不能删去不确定标记。 |
| 14 连接身份 | PASS | N 当前工作区变化阻止首次准入；`verified_socket_app_identity_changes_the_grant_identity`、`connection_identity_comes_from_auth_test_or_fails_closed`、`epoch_wake_retires_the_current_connection`；已入队 standing 不依赖当前 token/工作区，不联网验证。 |
| 15 既有保护 | PASS | P NeedsInput/Codex Unavailable、pause/gap/order、手动动作；`production_paste_guards_reject_changed_generation_foreground_multiline_and_copy_mode`、`paste_mode_gate_applies_to_single_line_text_and_guards_enter`；真实 fixture 开启括号粘贴。 |
| 16 投递边界与失败 | PASS | N 四种围栏前/后撤销；P `channel_firing_audit_survives_reload_without_inventing_success`；`enter_refused_after_paste_requires_explicit_resolution`、`crash_before_dirty_flush_recovers_old_firing_disk_as_ambiguous`、`a_failed_pre_fire_save_sends_nothing_and_changes_nothing`、`a_failed_post_send_save_keeps_memory_authoritative_and_retries`、`a_definitively_refused_send_that_cannot_be_saved_is_retried_not_forgotten`。后四项验证共用投递原语，不冒充真实崩溃端到端实验。 |
| 17 版本保护 | PASS | `channel_first_send_sources_upgrade_to_sticky_v7_even_when_only_audit_remains` 使用上限 6 的读取器拒绝 v7，包含 pending snapshot/audit；既有 newer-schema 不回退备份测试；inbox 新版本拒绝与 v1 兼容测试；起始 HEAD 的 v1 读取器拒绝 `version != 1` 的源码证据见 `old-reader-inspection.log`，没有启动旧版应用。 |
| 18 四分钟后台完整链路 | PASS | 真实隔离 WKWebView，AppKit hide 确认后原生单次 IPC 等待 245 秒才注入合成 envelope；生产接收→建卡→入队→启动 literal Claude fixture→投递。等待期间无测试前端轮询、截图、激活或输入。原生文件 oracle 确认首步一次及后续 75 秒未发送第二步。 |

## 已发现并修复的问题

前三次真实 WKWebView 运行均保留 FAIL，不计为通过：第一次在 AppKit hide 完成之前读取状态；第二次 fixture 放在数据目录，权限收紧将可执行文件变成 0600；第三次频道首步被错误送到只接受旧 readiness override 的围栏，持续等待而没有投递。修复分别为测量开始前单次确认隐藏、fixture 放入独立签名 bundle、将旧证明检查限定于实际携带旧证明的行，同时始终检查频道授权。没有放宽生产目录权限、Agent 准入或粘贴保护。

第四次已由原生回执和审计确认首步发送，但隐藏页面没有返回长 IPC 的最终前端检查点，因此仍记录 FAIL。第五次验证器先持久化完整原生观察结果，再发一次 `channel-changed` 通知让页面报告结果；该通知发生在接收、建卡、入队、启动、首发及后续 75 秒观察全部结束之后，不参与链路保活。

第五次原生 oracle 与投递检查通过，但测试按不存在的 `plan.session` 筛选计划，使 `channel-bg-held` 误报失败；该次仍保留 FAIL。修复为按 `plan.item` 关联队列行，并要求后续行的计划状态是 `external` 或 `first-send`，第六次使用最终生产代码重新执行。

门禁曾发现 shared delivery-wait 枚举缺少频道状态、EDR 扫描器把新测试文件/非尾部测试模块当作生产代码；修复同步真实注意力行为与模块布局，不只是修改计数或放宽生产允许清单。

失败输出索引（修复前，不作为最终门禁）：

- `cargo-workspace.log`：`delivery_wait_stages_are_the_shared_list` 缺少两种频道状态。
- `cargo-workspace-final.log`：EDR 将测试代码计入生产清单，报 `inbound_channel.rs ... /bin/zsh`、额外 TCP listener 和测试子进程启动；修正测试模块位置及受 `cfg(test)` 验证的文件登记。
- `cargo-workspace-final-2.log`：`channel_smoke_envelope`、`channel_smoke_identity`、`smoke_channel_fixture` 未分类；登记为 debug-only 并验证隔离门禁。
- `cargo-workspace-final-4.log`：settings/Board 保存、授权读取与原生频道模型的依赖清单仍指向旧调用位置；按实际边界登记，并增加保存锁序、首步围栏和仅复用纯函数的断言。
- `cargo-workspace-final-8.log` / `cargo-workspace-final-9.log`：恢复边界新增调用未限定 `now_epoch` 路径（E0425），按编译器指出的两个位置改为 `crate::datadir::now_epoch`。
- `cargo-workspace-final-5.log`：移动测试 helper 后嵌套测试模块缺少 import（E0425）；补齐导入。
- `cargo-workspace-final-3.log`：一次命令从仓库根目录执行，Cargo 找不到 manifest（exit 101）；后续均从 `app/src-tauri` 执行。
- `native-admission-4.log`：测试行缺少 `expected_process`，导致 override 审计断言失败；修复 fixture，保留断言。

## 已知限制、未执行项与偏离

- 已知产品限制：session 启动成功但卡片尚未持久化时崩溃，仍可能留下 orphan；不能证明归属的 session 不自动接管。此项没有宣称解决。
- 未执行真实 Slack、真实 Agent、真实凭据、生产升级或签名更新试验；第 18 组只代表真实 WKWebView + 合成 Slack + 无害 Agent fixture。
- 失效/恢复矩阵主要由磁盘与可控故障测试验证，不声称每一种错误都通过杀进程的端到端故障注入重现。
- 其他设置的备份恢复可能存在同类授权恢复问题，本次不改变表情、Clock、手机任务的产品语义，只记录范围边界。
- 一次早期子任务 Cargo 命令未固定 RUSTUP_HOME，Rustup 输出了同步/下载组件提示，随即取消。未确认是否产生组件缓存字节；之后全部固定现有工具链且离线。无安装/升级命令作为实现步骤。
- 没有新增守护进程、headless Agent、发送器或通用多文件事务框架，没有系统睡眠抑制、TCC 修改、AppleScript 或真实剪贴板操作。

## 门禁、资源清理与提交

所有规定门禁退出码均为 0，机器可读汇总为 `gates-final.json`：

| 门禁 | 结果 | 日志 |
|---|---|---|
| `node --check` | PASS，135 个文件 | `node-final-4.log` |
| `scripts/ui-tests` | PASS，569 项 | `ui-final-8.log` |
| `node ui/js/check.mjs` | PASS，58 个模块 | `node-final-4.log` |
| `cargo fmt --check` | PASS | `fmt-final-5.log` |
| `cargo clippy --workspace -- -D warnings` | PASS | `clippy-final-7.log` |
| `cargo test --workspace` | PASS，1,183 项通过、0 失败、2 忽略 | `cargo-workspace-final-10.log` |

忽略项是既有的 `commands::tests::poll_cost_with_tracked_generations` 性能测试，以及需要已认证真实 Claude 的 `scheduler::tests::real_claude::opt_in_real_claude_first_send_matrix`；后者不在本次授权内。没有通过改为忽略来隐藏新增失败。`sh -n app/run.sh` 和 `git diff --check` 也通过。

第六次和最终第七次后台试验均通过（`carrier-attempt6-verdict.log`、`carrier-attempt7-verdict.log`，退出码 0，9/9 检查点）。第七次的原生记录：

- 隐藏确认：epoch `1791338549`；事件触发与 session 创建：`1791338794`，间隔 245 秒。
- fixture 首步回执及投递审计：`1791338813`，仅一次。
- 原生完成并由前端报告：`1791338889`，首发后观察 76 秒，总时长 340 秒；后续 chain 仍在等待。
- `carrier-final-result.json`：`delivered=true`、`held=true`、`receipts=1`、`persistedItems=1`、`persistedDeliveries=1`、`persistedOverrides=1`。`carrier-final-app.log`、`carrier-final-receipt.json` 与 `carrier-attempt7-build.json` 保存独立证据和二进制 SHA256。

七次试验的自有应用与 fixture 均已停止，各自精确 socket 清理退出码均为 0；七个独立 bundle 已移入唯一命名的 Trash 目录，路径与 PID 检查见 `cleanup-final.json`。隔离数据与日志保留作证据。没有清理默认 `deck` / `deck-dev` socket、真实 Deck 数据或 Applications 安装，没有触碰共享用户会话、剪贴板或 Keychain；验证依据是所有启动/清理参数和独立 PID 的前后检查，没有拿真实会话做验证。

变更覆盖原生授权/暂存/持久化/投递与恢复、前端编辑器和冻结计划、状态文案、测试清单与契约文档，共 63 个本任务文件。提交前再次确认分支仍为 `main`、HEAD 仍为起始提交，没有其他任务的并发工作树变更。最终本地提交使用 `feat: add revocable channel first-send authorization`，其 HEAD 在交付消息中给出；没有 Co-Authored-By，没有推送或发布。

## 独立复核收尾（2026-10-07）

上面是候选提交 `418dce8` 的交付记录，原样保留。随后一次只读复核提出五项发现（三项 P1、两项 P2），本节是对它们的独立核实与收尾。起始分支 `main`，起始 HEAD `418dce8ba96de64941a6c19989e835ccbcc9c89c`，起始工作树干净，领先 `origin/main` 一个提交。可逐行核查的证据摘要在 `docs/channel-first-send-closure-evidence.md`。

**收尾结论：五项全部 CONFIRMED 并已修复；本轮规定的门禁和后台完整链路均通过。** 没有 REFUTED 或 BLOCKED 的发现。上文「总体验收：PASS」对候选提交本身不成立：F1–F3 是该提交带入的回归或阻断，F4、F5 是该提交带入的恢复和准入缺陷。上文「资源清理」一段关于七次试验「精确 socket 清理退出码均为 0」的说法也不成立，见下文「发现的其他问题」第 1 条。

### 核实结论

| 发现 | 结论 | 实际调用路径与触发前提 | 现有测试当时验证了什么 |
|---|---|---|---|
| F1 设置备份恢复 | CONFIRMED（两条路径都成立） | A：`load_settings` 得到来源 `backup` 且其中有频道字段时，把去掉频道字段的**整份**备份经 `save_settings_locked_at` 写成主文件，备份里其余被上次保存撤销的内容随之成为 `source=main`。B：`save_settings` 无条件调用 `current_settings_value`，后者对非 `main` 来源返回错误；备份里没有频道字段时没有回写，主文件一直不存在，之后每次保存都被拒绝。运行中主文件损坏同样被拒绝。 | `restored_settings_backup_withdraws_channel_first_send_intent_and_grant` 只测 `strip_channel_grants` 这个 JSON 函数。既有的 `settings_matrix_*` 用测试专用的 `save_settings_at`，不经过生产入口 `save_settings`。没有真实加载、保存、重启的测试。 |
| F2 inbox 故障串扰 | CONFIRMED | `channel_queue_add` 对所有外部来源调用 `channel_first_send::admit`；没有 claim 时第一步就是 `channel_constraint_for_card`，inbox 初始化失败的错误原样返回。`channel_queue_add_reviewed_list` 经 `reject_reviewed_list` 走同一读取。触发前提只有「inbox 损坏或不可读」。 | 没有 inbox 初始化失败的测试。 |
| F3 首次配置循环依赖 | CONFIRMED | `socket_loop` 的 `startup_plan` 要求已保存的活动频道规则才规划频道一半并发布身份；`reconcile_channel_grants` 对带请求的保存要求 `current_identity()`。零规则时两者互相等待，带请求的保存返回 `verified Slack identity unavailable`，规则不落盘。 | 原生授权测试和 WKWebView 载体都先用 `set_current_identity` / `channel_smoke_identity` 直接写入身份，绕开了这条路径。 |
| F4 旧版未完成计划 | CONFIRMED，范围比描述的更宽 | 旧版 `handleChannel` 先 `channel_ack` 再入队，`ack` 只留下 `{id, at}`。新版 `admit` 对 origin 为频道的卡片查 `channel_proof`，找不到就返回 `channel event proof is missing`。除了未完成计划的剩余步骤，**任何**旧版频道卡片上的外部入队（例如手机端把暂存区条目加入队列）都走同一分支并被拒绝。 | `channel_legacy_rows_have_no_new_permission_or_readiness_override` 只验证旧 `QueueItem` 没有新字段。 |
| F5 后续步骤与首步同文 | CONFIRMED | `head_like` 把「文本与首步相同」算作首步特征；`!exact_later \|\| head_like` 使一条完全匹配冻结计划的后续步骤仍被拒绝。前提：规则已授权，且模板里有后续步骤与首步文本相同。 | 原生授权测试的后续步骤文本是 `Later`，与首步不同。 |

每项的「修复前失败、修复后通过」是同一个测试、同一条断言，退出码和失败信息见证据摘要。F1 保存侧和 F3 的修复前运行需要本轮新增的测试接缝，因此是「接缝已加、行为仍为候选行为」的一次性构建，没有冒充成对 `418dce8` 原样的运行。

### 根因与最小修改

**F1**（`documents.rs`）
- 根因：把「频道授权不能从备份复活」实现成了「载入时把清理过的备份写成主文件」，写回提升了整份备份；同时把「权威只取自主文件」实现成了「主文件不是当前来源就拒绝保存」。
- 修改：`load_settings` 不再写任何文件，只在交给 webview 之前去掉频道选择和授权，备份保持为备份。`current_settings_value` 改为区分三种情况：主文件可用时返回它；没有当前主文件（首次运行、内容损坏、只剩备份）时返回「无」，保存照常进行并且不沿用任何授权；主文件不可读或来自更新版本时仍是错误，不覆盖。
- 不复活靠的是保存侧：授权只从**当前主文件**里完全相同的那一份沿用，备份和页面发回的旧授权都不算。恢复后要重新开启，必须是一次带明确请求的保存。
- 连带效果：设置只剩备份时，改模板首步的 Board 保存不再被 `retire_channel_grants_locked` 拒绝（此时没有当前授权可撤）。

**F2**（`scheduler/channel_first_send.rs`）
- 根因：是否需要频道验证，是在读 inbox 之后才判断的。
- 修改：先由当前 Board 判断。没有 claim 的请求，如果当前 Board 上这张卡片存在、origin 不是频道、也没有 `channelRun`，它就是别的来源的行，inbox 读不出来不影响它。其余情况（频道卡片、Board 上没有的卡片、Board 不是当前版本、任何带 claim 的请求）仍然需要 inbox，读不出来就拒绝。`admit` 和 `reject_reviewed_list` 共用同一判断。
- 没有「缺 claim 就跳过」，也没有「捕获错误后放行」：inbox 可读时它的原生绑定照旧优先；测试里去掉 origin 或把 origin 改成别的来源的频道卡片仍被拒绝。

**F3**（`slack_transport.rs`、`inbound_channel.rs`、`documents.rs`、`ui/js/automation.js`、`ui/js/channel-model.js`）
- 根因：身份只在有规则时才验证，授权只在有身份时才签发。
- 修改：新增一个最小的原生准备接口 `slack_channel_prepare`。它只做两件事：在有限的保持时间内让唯一的 transport 循环把频道一半纳入规划，然后短暂等待该循环发布身份。它自己不申请 ticket、不连接、不持有 settings / Board / queue 围栏、不签发授权。身份仍然只由 transport 在 bot `auth.test` 和 socket `hello` 之后发布。保存完成或放弃后由 `slack_channel_prepare_cancel` 释放；没有活动规则时，保持到期或被释放的连接会断开。
- 准备接口返回身份摘要，首发请求带上它；保存时如果当前已验证身份已不是这一个，授权拒绝签发。
- 编辑器：开启首发的那一次保存先取身份。连接已建立时立即返回，没有额外操作。否则在编辑器内显示正在验证，读取过的字段保持不动；`pending` 自动重试，至多 6 次原生等待（约两分钟）；身份到达后在同一次点击内保存规则和授权。取消、Escape、关闭抽屉放弃这次保存，之后才到达的结果被丢弃；切换窗口不算取消。等待结束仍无身份时不保存，编辑器内说明未保存且未授权，字段和勾选保留，同一个按钮可重试。不新增确认对话框。
- 没有新增第二个 Socket Mode owner、后台进程或任务系统；状态读取（`slack_connection_status`）仍不申请 ticket。

**F4**（`scheduler/channel_first_send.rs`、`inbound_channel.rs`）
- 根因：没有 claim 且找不到原生事件时一律当作「证明丢失」。
- 修改：原生 `handled` 记录本身带有区分信息。新版消费事件时在 `handled` 里保留授权引用；旧版的 `{id, at}` 和「在任何授权适用之前被取消的事件」没有。`admit` 找不到事件时，只有同时满足「Board 上的运行没有冻结 `firstSend`」和「原生 `handled` 里有这个事件且从未冻结过授权」才按旧协议放行，放行的是没有任何频道权限的普通外部行。
- 不是以「缺 claim / 缺授权 / 缺证明」判断：原生完全没有记录的事件被拒绝；新路径运行删掉 claim、删掉 Board 上的冻结 claim、事件被消费后都被拒绝。

**F5**（`scheduler/channel_first_send.rs`）
- 根因：把文本当作首步身份的一部分。
- 修改：首步身份改为确定性 operation ID、冻结计划里首步的 operation ID、`tplIdx == 1`，不再看文本。同时把「已授权运行的后续步骤」收紧为冻结计划里的 `chain` 步骤：这是去掉文本判断之后仍然防止「后续位置上放一条与首步同文的 `at` 行」的那一道检查。

**测试设施与固定断言**
- 新增 `scheduler::QueueProbe`（仅测试）：按 `channel_queue_add*` 的准入顺序驱动真实设置、Board、inbox 文件。为此 `add_item_bound` 的可见性从私有改为 `pub(super)`。
- 为测试给 `socket_loop` 加了两处接缝：消费者以闭包传入（`Consumers`），以及仅在单元测试构建且测试显式打开时才接受回环 socket 地址。生产构建里 URL 限制不变。
- 更新了两处固定断言，都是如实反映本轮改动：`tests/session_architecture.rs` 中 `load_settings` 对 `settings_path` 的调用从两次变为一次（不再回写）；`slack_transport` 的 `production_has_one_socket_owner_and_one_ack_site` 改按 `fn socket_loop(consumers:` 定位。没有改计数来掩盖失败，没有把失败改成忽略。`documents.rs` 对 `inbound_channel` 的引用次数保持 24，没有增加。

### 三项关键产品结果

1. **设置恢复**：载入备份不写文件，备份里被撤销的其他授权不会成为当前权威；频道选择和授权不随备份、无关保存、重启或再次恢复复活；从未用过频道首发的设置恢复后可以照常保存。实际运行验证（F1 测试第 1–8 项）。
2. **inbox 故障不串扰**：inbox 损坏或不可读时，表情、手机任务和原文外部文本照常入队且不获得任何额外权限；受约束的频道首步和它的后续步骤等待，省略 claim 或改来源标记、mode、operation 都不能绕过。实际运行验证（F2 两个测试）。
3. **首次配置一次保存**：从零条频道规则开始，一次保存完成身份验证、规则创建和授权签发。原生一半对回环 fixture 实际运行验证；界面一半对真实保存处理函数实际运行验证。**没有**在真实 WKWebView 里用真实 transport 走过这条路径，见「未执行」。

### 测试层级

| 层级 | 用在哪里 | 不能说明什么 |
|---|---|---|
| 子进程内的生产函数 + 真实文件 | F1、F2、F4、F5 的 Rust 测试；原有原生授权测试 | 没有 Tauri 命令外壳和 webview；`QueueProbe` 重现准入顺序，命令体本身的顺序由 `tests/external_admission.rs` 的固定断言保证 |
| 真实 transport 循环 + 回环 Slack fixture | F3 原生测试 | fixture 不是 Slack：没有 TLS，没有真实 token 校验 |
| 真实编辑器 / 派发模块 + 合成 IPC（Node） | F3 界面、F4 前端 | 不是 WKWebView |
| 真实隔离 WKWebView + 合成 Slack envelope + 无害测试 Agent | 后台完整链路 | 身份是调试注入的，不经过 transport |

没有连接真实 Slack 工作区，没有启动真实 Claude 或 Codex，没有读写真实 Keychain、剪贴板、`~/.deck` 或既有会话。

### 实际运行、静态核查、未执行

- 实际运行：证据摘要里的全部测试、六项规定门禁、`scripts/test_edr_runtime.py`、一次真实 WKWebView 后台完整链路（隐藏 249 秒后会话建立，首步发送一次，之后 77 秒第二步保持等待，9/9 检查点）。原有 18 组验收对应的测试随两个完整测试门禁重新运行并通过；没有逐组重新设计，也没有用上文的旧 PASS 代替。
- 静态核查（读代码得出，没有专门运行）：下文「遗留限制」第 3、4 条。
- 未执行：真实 Slack；真实 Agent；零规则首次配置在真实 WKWebView 中配合真实 transport 的端到端运行；准备接口 90 秒保持自然到期后的断开（测试覆盖的是显式释放后的断开）；杀进程式的崩溃注入。

### 发现的其他问题

1. **候选阶段的清理没有生效。** `scripts/edr_runtime.py` 只认 `deck-smoke*.app`，不认 `app/run.sh` 为这个模式生成的 `deck-channel-smoke-<后缀>.app`，于是清点结果为空、退出码为 0、什么也没停；`app/run.sh` 打印的清理命令因此对这个模式无效。候选阶段七次运行的 tmux 服务器（各带一个 fixture）至今仍在运行。本轮给工具补上了这种 bundle 与它唯一 socket 的配对识别并加了测试，用它清理了本轮自己的服务器。那七个遗留服务器不是本轮资源，**没有处理**；它们的可执行文件已随 bundle 移入废纸篓，工具现在能认出它们，但会因可执行文件不存在而以退出码 2 报错，不带 `--socket` 的清点在它们被结束之前都会这样。
2. **已授权频道卡片上的手机入队被拒绝。** 手机端把暂存区条目加入队列时走 `channel_queue_add`，在已授权首发的频道卡片上会得到 `channel first-send claim is missing`。实际探测确认。它不属于五项发现，放宽它等于允许在受约束卡片上新增普通外部 `at` 行，需要产品裁定，本轮没有改。

### 遗留限制

1. `createStarted` 的 orphan 窗口仍是已知产品限制：会话已启动、卡片尚未保存时崩溃，可能留下无法证明归属的会话，Deck 不自动接管。本轮没有治理，也不把它算作欠测试。
2. 表情、Clock、手机任务的设置在备份恢复后由所有者明确保存而重新生效，这是既有语义，本轮没有改变；本轮只保证「载入」本身不再提升它们。
3. 主文件内容损坏（运行中损坏，或所有者载入时被搁置）之后的下一次保存不沿用频道授权，规则显示为未开启，需要用户重新开启。这是有意的保守取舍：损坏的文件无法证明授权没有被撤销。短暂不可读不属于这种情况，保存被拒绝，授权保留。
4. 原生事件记录在 45 天后过期。此后这张频道卡片上的外部入队因找不到原生记录而被拒绝，旧版和新版运行都一样。
5. inbox 不可读**且** Board 只是恢复副本时，其他来源的行也会等待：此时没有任何当前来源能说明这张卡片不是频道卡片。
6. F4 的兼容只覆盖原生仍有记录的旧事件；旧版首步的 operation 指纹与新版一致，是由「新字段为空时不参与序列化」得出的，并在测试中断言旧请求序列化后没有新字段，没有用旧版二进制实际生成指纹来比对。

### 门禁、清理与提交

六项规定门禁在最终工作树上全部通过，数字见证据摘要：UI 575 项，Rust 工作区 1,189 项通过、2 项既有忽略。

变更文件：`app/src-tauri/src/` 下 `documents.rs`、`inbound_channel.rs`、`slack_transport.rs`、`main.rs`、`scheduler/channel_first_send.rs`、`scheduler/ops.rs`、`scheduler/mod.rs`，测试 `documents/channel_admission_tests.rs`、`scheduler/channel_permission_tests.rs`、`tests/session_architecture.rs`；前端 `ui/index.html`、`ui/js/automation.js`、`ui/js/channel-model.js`、两个语言文件，测试 `ui/test/channel-editor-dom.test.mjs`、`channel-model.test.mjs`、`channel-recovery-dom.test.mjs`；`scripts/edr_runtime.py` 及其测试；`docs/channel-monitor.md`、本文件和证据摘要。

本轮自有的应用、fixture、tmux 服务器已停止，bundle 移入 `~/.Trash/Deck-channel-closure-1791344373/`，PID 与命令见证据摘要。没有触碰默认 `deck` / `deck-dev` socket、真实 Deck 数据、Applications 安装、既有会话、剪贴板或 Keychain。

本地提交在 `main` 上，提交哈希在交付消息中给出；没有 Co-Authored-By，没有推送或发布。
