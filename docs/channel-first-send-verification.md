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
