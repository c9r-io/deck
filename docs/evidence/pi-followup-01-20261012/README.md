# Pi v0.8.0 Follow-up 01 实施证据（执行者提交，待独立验收）

对应 Follow-up 01（`docs/design/pi-v0.8.0/follow-up-01.md`）与负责人的独立验收（`docs/evidence/pi-acceptance-20261012/README.md`）；两者均为负责人本地文档，不在版本控制内。本文件由执行 Agent 撰写，是提交给负责人复核的材料，不是验收结论：下面所有“自评”都需要独立验收确认，执行者不自签最终通过。

## 1. 被测构建与证据的对应关系

| 项 | 值 |
| --- | --- |
| 基线 | `1bdcf57`；上一轮被拒的提交 `7e4f8d7` |
| **被测代码提交** | `eb16c7123f001149ec3d3aa6c4e8f5b78bc45d1a`（本地 `main`，未 push） |
| 本文件所在提交 | 紧接在被测代码提交之后，只增加 `docs/evidence/pi-followup-01-20261012/` 这一个目录，不改动任何被 `scripts/pi_build_identity.py` 计入的源文件；在本文件所在提交上运行 `python3 scripts/pi_build_identity.py`，`source_sha256` 仍是下面这个值 |
| 工作区 | 每次运行开始时 `dirty=false`（native、三次真实窗口、门禁各自记录在其结果里） |
| `source_sha256` | `6388ddd457ae330231a6506df19119a0e30b98ad2026393f9b73091402511549`（324 个文件；算法见 `scripts/pi_build_identity.py` 头部） |
| 测试可执行文件 | `target/debug/deps/deck_app-5342d33110f51628`，SHA-256 `1b47b56238f6ea1f58b24d794d95f81b83ddd238a5c17191fabc4765ad141fd1`（native 运行所用，亦记录在 `native-result.json` 的 `facts.build`） |
| debug 主程序（WK 载体的可执行文件） | 构建产物 `target/debug/deck-app` 的 SHA-256 `0a0f9b75865a2f15509f97b4f0d5464a5ef6630912f79f6d78950737ec701f87`。载体在 ad-hoc 签名**之前**逐字节等于构建产物（这是每次运行的一项检查）；签名只改写主程序，所以载体里“实际运行的文件”的哈希与构建产物的哈希分别记录在各 `wk-*.json` 的 `facts.carrier.files` 与 `facts.carrier.as_built_sha256` |
| observer 四个文件 | `observer.ts` `31a96a37…abbd22`、`signal.ts` `8f86651d…597f1`、`lifecycle.ts` `13e6940d…3c45a`、`both.ts` `906761d9…55a61`（完整值在 `native-result.json` 与 `wk-*.json`） |
| Pi 运行时 | npm 安装的 Pi 1.1.0（Node.js v24.19.0）；官方 standalone 1.1.0（`pi-darwin-arm64.tar.gz`，校验和与上一轮 `identity.json` 相同）。版本与文件哈希见 `native-result.json` 的 `facts.artifacts` 与 `wk-*.json` 的 `facts.runtime` |
| 运行位置 | Unit/Integration/Native/门禁：开发机，macOS 27.0.1（26A434，arm64）；真实窗口（WKWebView）：隔离的测试机，同为 macOS 27.0.1（26A434，arm64），其已安装的 Deck 与数据前后不变 |

本轮在 `7e4f8d7` 之后的提交（均为本地提交）：

| 提交 | 内容 |
| --- | --- |
| `b59e88d` | 负责人的验收文档与 Follow-up，原样提交（未改一字） |
| `34082fd` | F01、F02、F03、F06 的产品修复与单元/集成测试 |
| `599095b` | native 驱动：构建身份、`exec_peer`、`capabilities`、`unsupported_ids` |
| `0d826fb` | 真实窗口：新模式 `pi-layout`，`pi-live` 打开真实对话框并新增检查点 |
| `a18aa8a` | 更正上一轮证据中 A12、B02 的错误判定 |
| `4427630` | F03 补充修复：能力少于声明的连接，在 `accepted` 之前先被告知可用能力 |
| `ef37976` | native：每个运行时 idle/active 全覆盖与真实“在途帧”；真实窗口：Needs attention 视图 |
| `eb16c71` | 真实窗口操作员：载体与构建产物在签名前逐字节比对并入检查；“可执行文件不得旧于源文件”的检查覆盖被嵌入的 smoke 页面 |

最后三个提交的由来：`a18aa8a` 上的完整门禁全绿之后，执行者在最终 diff 审查与证据自查中又发现三处不足并补上——能力少于声明的连接会让一个 session id 过线一次（`4427630`）；真实 Pi 的撤销矩阵与 Needs attention 视图覆盖不全（`ef37976`）；载体清单里一项记录因签名而恒为“否”、容易被误读（`eb16c71`）。随后**全部证据在 `eb16c71` 上重新生成**；更早提交上的运行结果一律不作为本文件的证据（它们列在 §7）。

## 2. 判定一览（执行者自评，待独立验收）

| 项 | 自评 | 一句话依据 |
| --- | --- | --- |
| F01 跨 exec 的旧实例归属 | PASS | 以内核的 exec/exit 报告（kqueue `EVFILT_PROC`）补足运行实例连续性；真实同 PID exec 的旧连接帧被拒 `peer-executed` |
| F02 不支持的 Session ID | PASS | 闭合的 `unsupported-native-id` 贯通 observer→wire→后端→poll→卡片；同 lease 不可恢复，Signal 不受影响 |
| F03 两个能力独立撤销 | PASS | stream 内收紧，保留 lease 与另一能力；两种运行时 × idle/active × 两个开关均真实验证，含真实在途帧 |
| F04 真实可见 UI 门禁 | PASS（两项按范围 BLOCKED） | 真实窗口打开 Settings、卡片、Needs attention、“Why?”、Restart Review；真实 Claude 重入与通知发布单列 BLOCKED |
| F05 证据对应最终构建 | PASS | 构建身份、可执行文件哈希、载体清单、运行时、命令、exit、逐场景结果、清理均入证据；A12/B02 已更正 |
| F06 共享 listener 首行准入有界 | PASS | 每连接绝对首行期限、32 个在读上限、有界队列与拒绝；真实 socket 上的静默/滴流不再阻塞其他协议 |

| 总判定 | 自评 | 说明 |
| --- | --- | --- |
| Design | PASS（含需裁决点，见 §8） | 所有补充都在 Follow-up 明确批准的范围内；新增的 wire 元素与取舍列在 §8，由负责人裁决 |
| Implementation | PASS | F01–F06 均已关闭并有对应证据 |
| P1a Signal | PASS | exec 后旧 lease 不再可写；能力互不影响；入口有界；UI 门禁完成 |
| 完整 P1 | BLOCKED | Pi 1.1.0 仍无可靠的 needs-input/clear 原语；本轮没有新增上游解除证据，也没有订阅 `ui_prompt` |
| P2 Lifecycle | PASS | 超长 ID 显式降级且保留一致性失败；两个能力独立撤销；standalone 的进程内会话切换已实测 |
| P3 Re-entry | BLOCKED | Pi 的供应方运行代码身份未被证明；`PI.reentry` 仍为 `None`。拒绝路径的测试通过**不是** P3 PASS |
| Security | PASS（残余见 §8） | 已复现的实例连续性缺口关闭；未授予任何自动输入或重入权限 |
| Regression | PASS | 完整门禁在被测提交上逐步 exit 0（§5）；`test-compat-candidate` 需 macOS 14，UNRUN |
| Native E2E | PASS（范围内）；其余 UNRUN/BLOCKED 见 §10 | 真实 Pi 两种运行时的完整矩阵 269 项通过；真实窗口三次运行通过 |
| Product Constitution | PASS | 静默丢弃改为显式、闭合的不可用说明；无自动卡片移动、无新权限、无 Agent 语义重建 |

## 3. 逐项说明

### F01 修复跨 exec 的旧实例归属（高）

**根因。** lease 与“这个前台程序是已证明的 Pi observer”的证据只绑定 `ForegroundGeneration`（pid 与 birth）。`execve` 与 `posix_spawn` 的就地执行形式都保留 pid 和 birth，所以 `transport::verified`、poll 的 `reconcile` 和 `agent_status` 的每个读取方都继续把新映像当作原 observer。原测试 `transport/tests.rs:580` 改变的是 birth，那是 PID 复用，不是 exec。这也是原详细设计的缺口（只规定了 pid/birth）。

**原语的选择（先实验，后实现）。** 在 macOS 27.0.1 上用根内自编译的 C 程序实测了七种情形，原始记录在 [`exec-primitive/`](exec-primitive/)（`peer.c`、`prim.py`、`primitive.json`）：

| 情形 | kqueue `EVFILT_PROC` | audit token 的 pidversion | birth |
| --- | --- | --- | --- |
| `execv` 到另一映像（exec-other） | `NOTE_EXEC` | 变 | 不变 |
| `execv` 到同一映像（exec-self） | `NOTE_EXEC` | 变 | 不变 |
| `posix_spawn` 就地执行（spawn-setexec） | `NOTE_EXEC` | 变 | 不变 |
| fork，子进程在继承的连接上写（fork-child-writes） | 仅 `NOTE_FORK`（实验额外订阅） | 父不变 | 不变 |
| fork，子进程 exec（fork-child-execs） | 仅 `NOTE_FORK` | 父不变 | 不变 |
| 运行中的程序在磁盘上被替换（file-replaced） | 无 | 不变 | 不变 |
| 非子进程（detached）exec | `NOTE_EXEC` | 变 | 不变 |

结论：`EVFILT_PROC` 的 `NOTE_EXEC | NOTE_EXIT` 是公开接口，不需要 task port、特权或 entitlement，不启动任何进程（符合 `procinfo/code_identity.rs` 头部“不取任何 task port”与 EDR-quiet 契约），能区分 exec、fork、磁盘文件替换与新进程 birth。实验同时显示 `LOCAL_PEERPID`/`LOCAL_PEERTOKEN` 会跟随**最后一个使用该 socket 的进程**（子进程一写就变），所以没有采用“每帧重读 peer token”作为连续性事实；pidversion 的变化只作为佐证记录。`PROC_PIDUNIQIDENTIFIERINFO` 不在公开 SDK 中，未使用。

**修复**（`34082fd`）：

- `app/src-tauri/src/procinfo.rs`：`InstanceWatch` 与 `Continuity::{Same, Executed, Exited, Unreadable}`。全应用一个 kqueue（一个描述符）；第一次报告即定论，不会变回 `Same`；读取失败是 `Unreadable`，调用方一律 fail-closed。
- `app/src-tauri/src/agent_pi/transport.rs`：listener 读完首行并取得内核的 peer pid 后立即登记 watch（`Service::hand`）；hello 绑定（没有 watch 即 `no-instance`；同 generation 的旧 lease 若实例已变，先结束）、**每帧** commit（`verified`，在其他内核读取之后最后询问）、poll 的 `reconcile_with` 都询问它。exec 是 `peer-executed`，进程已退出是 `generation-mismatch`，内核不可问是 `no-instance`。
- `app/src-tauri/src/agent_status.rs`：hello 证明的事实与 lease 写入的 word 都带着同一个 watch 保存（`PiSeen`、`PiWriter`），每个读取方先问它：poll 的 evidence、调度器的两个问题（`pi_proven`、`pi_foreground`）、Restart Review、word 的投影（`projected`、`reported_by`）。exec 之后那个 pid 里没有“已识别的 Pi”，也没有 word、coverage 或 session；内核不可问时保留识别（它只会**增加** hold），但绝不说 `connected`。
- 这只回答“是否仍是原运行实例”，不回答“是谁的程序”：不比较进程自报名称、路径文本、Session ID 或 observer 自报的任何值；不涉及代码签名；没有摘要白名单；`agent_pi::PI.reentry` 仍为 `None`。

**测试**（命令与 exit 见 §4/§5；下列名字都在被测提交里）：

| 要求 | 层 | 测试 | 期望 → 实测 |
| --- | --- | --- | --- |
| 真实同 PID 的不同 image exec | Integration（真实 tmux pane、真实 C peer、production listener 与 transport） | `agent_pi::transport::tests::a_real_exec_inside_the_observer_s_pid_ends_its_lease` | exec 后的帧被拒、lease 结束 → 通过 |
| 同上，对应负责人的反例 | Native | `exec_peer`：hello accepted、working ACK、fork、exec 成另一映像（同 pid、同启动时刻）、新映像在保留的连接上发 seq=2 turn-done | `refused peer-executed`；poll 不再有 word、coverage、Pi hold → 通过 |
| 保留 FD 的迟到 frame | Unit + Native | `a_frame_on_a_connection_kept_across_an_exec_is_refused`；`exec_peer` | 被拒，什么也没写 → 通过 |
| 旧 callback / 旧证据 | Unit | `a_peer_that_executed_another_image_is_not_the_observer_any_more`；`commands` 的 poll 测试；`scheduler::tests::a_pi_target_is_recognized_from_facts_read_now`；`agent_reentry::tests::a_verified_pi_is_named_refused_and_never_started_whatever_is_approved` | exec 后 word/coverage/session/Pi 识别全部不再读出；Restart Review 不再把它当已证明的 Pi → 通过 |
| 静默 exec（不再发任何帧） | Native | `exec_peer`：第二个 pane 的 peer exec 后保持沉默 | 由 poll 的 reconcile 结束 lease，卡片不再说 Pi → 通过 |
| PID reuse | Unit | 原有 later-birth 测试；`a_peer_that_is_gone_or_cannot_be_asked_about_is_not_trusted` | `generation-mismatch` → 通过 |
| 读取失败安全撤销 | Unit | 同上（`Unreadable`）；`procinfo::tests::a_watch_is_of_one_process_and_none_without_one` | `no-instance`，不绑定或 lease 结束，卡片为 unavailable → 通过 |
| 区分 exec、磁盘文件替换、新 birth；fork 不算 exec | Unit（真实进程）+ Native | `procinfo::tests::an_exec_inside_one_pid_ends_the_instance_and_a_replaced_file_does_not`、`a_fork_and_a_child_s_exec_are_not_this_process_s_exec`；`exec_peer` 的 fork 与文件替换步骤 | 只有 exec 结束实例 → 通过 |
| 正常 Pi Node/Bun 不误拒 | Native + WK | 整个 native 矩阵与两次 `pi-live`（npm、standalone） | 真实 Pi 全程无 `pi-peer-executed`/`pi-no-instance` 拒绝（`native-scenarios.json`：这两个词只在 `exec_peer` 场景里被计数，共 2 次；`wk-live-*.applog.txt` 里没有）→ 通过 |

**剩余风险（需负责人裁决，执行者不自批）。**

1. watch 在 listener 读完 hello 之后登记。“写出 hello”与“登记”之间发生的 exec 不会被报告：lease 绑定到新映像。新映像本身就是该 pane 的前台进程，可以自己说 hello，所以这不带来新权限；被错误归属的只有 hello 里的自报声明（版本、能力）。之后每一帧都受 watch 约束。
2. 继承了连接副本的子进程仍可替父进程的 lease 写帧（fork 不是 exec）。这是原设计已公开的同 pane 残余，本轮没有扩大也没有关闭。
3. exec 之后识别被遗忘，新映像只有在自报名为 `pi` 时才被按名字 hold。它不是已证明的 Pi；通用的 Agent/首次交互 hold 不受影响。
4. kqueue 不可用时 Pi 观察整体不可用（fail-closed）。
5. 只在 macOS 27.0.1（arm64）上实测。

### F02 明示不支持的 Session ID 并保留一致性失败（高）

**根因。** `NativeId::Unsupported` 只调用 `forget_pi` 然后普通 ACK：记录被清空成“还没见过任何事件”，同一 lease 的下一条短 ID 被当作首次观察接受；observer 对超过 512 字符的 ID 直接返回 `null`。`transport/tests.rs:1223` 把这一错误行为写成了通过的测试。

**修复**（`34082fd`）：

- `agent_reentry/lifecycle.rs`：lease 的记录是 `PiNative::Known{id,last}` 或 `PiNative::Unsupported`。一旦 `Unsupported` 就保持到 lease 结束：之后任何 lifecycle 帧（包括短 ID）只被确认、不改变任何东西（`PiNoted::Ignored`），只有新的 lease 才能重新观察。`PiSession::{Observed, Unsupported}` 进入 poll（`pi_session`）。
- wire（`agent_pi.rs`）：第六种帧 `unavailable`，唯一取值 `unsupported-native-id`，属于 lifecycle 能力，不含 ID。
- observer（`resources/pi/observer.ts`）：ID 整体发送或完全不发送。超过 512 且形状合法 → 发送上述闭合词一次，此后本连接不再谈 session；超过 512 且不是 Pi session id 的形状 → 是 observer 自己的错误（结束 observer，不报告）；不截断、不 hash、不静默省略。Deck 一侧 257–512 字节的合法 ID 同样进入 `Unsupported`。非法语法仍按错误拒绝（`bad-frame`/`bad-lifecycle`），不与“合法但超限”混同。
- 日志只有闭合词与 session tag：`[agent-pi] session unsupported-native-id s=…`，没有 ID、长度或路径。
- 用户可见：`app/ui/js/pi-model.js`（`PI_SESSION = ['observed','unsupported']`）、中英文案 `signal.pi.sessionUnsupported`，显示在卡片的“Why?”里。上限 256 没有提高。
- 同一 stream 的 Signal（run 边界）不受影响。
- 原来把错误行为钉成 PASS 的测试已改写为 `an_id_this_build_does_not_keep_makes_sessions_unsupported_and_leaves_the_runs`。

**测试：**

| 要求 | 层 | 测试 | 实测 |
| --- | --- | --- | --- |
| 边界 256 / 257 / 512 / 513 | Unit（wire、transport、observer） | `agent_pi::tests::an_id_that_cannot_be_sent_is_one_closed_word_and_no_other_shape`；`transport::tests::an_id_this_build_does_not_keep_…`；`pi-observer.test.mjs` 的 “an id that does not fit a line is said in one closed word, once, and never passed over”、“…too long AND not a session id at all is an error…” | 通过 |
| 同上，真实 Pi | Native | `unsupported_ids`：两种运行时各以 256、257、512、513 字节的 `--session-id` 启动 | 256 完整观察；257/512/513 卡片为 `unsupported`；run 照常报告；没有任何拒绝 → 通过 |
| 正常 A → 超限 → B；超限 start → 短 snapshot；shutdown ID 不一致；新 lease 恢复 | Unit | `agent_reentry::lifecycle::tests::…::an_id_this_build_does_not_keep_makes_the_lease_unsupported_for_good`；transport 同名场景 | 同 lease 不恢复；不一致仍 `bad-lifecycle`；新 lease 重新观察 → 通过 |
| Signal-only 不访问 SessionManager | Unit + 源码扫描 | `pi-observer.test.mjs` “without the lifecycle capability the session manager is never asked”；`tests/edr_quiet.rs` 钉住 observer 的受保护读取 | 通过 |
| both 降级时 Signal 仍工作 | Unit + Native + WK | 上述 transport 测试；`unsupported_ids`；`pi-live` 的 `pi-unsupported` | 通过 |
| 真实 Pi / 生产 IPC 的可用性投影 | WK | `pi-live` 的 `pi-unsupported`：真实 Pi 以 300 字节 ID 启动，production `poll_sessions` 给出 `pi_session: "unsupported"`，卡片“Why?”显示对应文案（截图 `shots/npm-pilive-why-unsupported-en.jpg`、`shots/npm-pilive-why-unsupported-zh.jpg`、`shots/standalone-pilive-why-unsupported-zh.jpg`）；操作员核对 Pi 自己的日志确有 300 字节 ID，且 app.log 与数据目录里没有它的任何片段 | 通过（npm、standalone） |

**剩余风险。** `Unsupported` 粘在 lease 上是 Follow-up 批准的最小语义；Pi 的进程内会话切换会建立新 runtime 与新 lease，因此切到短 ID 的会话后会自动恢复观察（native 已覆盖）。

### F03 两个能力必须能独立撤销（高）

**根因。** `enabled_changed` 发现任一能力不再启用就结束整个 lease；`end` 同时清 Signal 与 lifecycle；observer 把 `capability-disabled` 当作永久结束；`mismatched` 只看存活的 lease，于是 observer 被结束后 Settings 又显示 `current`。

**修复**（`34082fd`，补充 `4427630`）：

- lease 区分 `loaded`（runtime 加载的，即 hello 的声明）、`usable`（本 lease 仍获准使用的：声明 ∩ 当前设置，只减不增）与 `told`（observer 最后被告知的）。没有第二个 listener、第二套连接或另存的配置布尔值。
- 关闭 Signal：只撤这个 lease 的 word 与 episode；关闭 lifecycle：只忘掉这个 lease 的 session。另一能力、lease、序号与未读状态都保留。两个都没有了才结束连接（`capability-disabled`）。
- 触发点两个：Settings 开关（立即，`settings_changed`）；以及每一帧的 commit（Pi 设置文件的状态一变就重读，所以用户手改文件也会收紧）。
- 告知 observer：`capabilities{signal,lifecycle}` 行（只会更窄）。已在途的帧回答 `revoked N`：序号用掉、不写入任何东西、stream 继续。hello 从未声明的能力的帧仍是伪造，结束连接。
- hello 声明多于当前设置（某个开关已关而该 Pi 还没 reload）：以交集绑定；**`capabilities` 行写在 `accepted` 之前**（`4427630`），observer 先收紧再获准发送，于是被撤销能力的帧——尤其是 session id——根本不会过线。交集为空则拒绝。
- observer：`may` 只减；丢了 lifecycle 之后不再触碰 session manager；本 runtime 之后的重连只声明剩下的能力；两个都丢了就不再连接。
- 重新打开能力不扩权（没有任何 ping/ACK 能把能力还回去），要等 Pi 自己 reload；`needs-reload` 现在覆盖两种情况：存活 stream 的 `usable` 与设置不一致；或已识别且仍在运行的 Pi 的 observer 因能力被结束（`pi_waits_for_reload`、`pi_reload_pending`）。
- 配置读取失败时能力集为空，按既有 fail-closed 契约结束连接。

**测试：**

| 要求 | 层 | 测试 | 实测 |
| --- | --- | --- | --- |
| both→signal、both→lifecycle，idle 与 active，Node 与 Bun 分别 | Native | `capabilities` 跑两轮，两种运行时互换位置：每个运行时在每个开关上都各有一次 idle（带未读结束）和一次运行中 | 剩余能力继续产出（同一 lease 上完成整轮 run / 观察到会话切换）；被禁用的能力没有任何写入；未读结束不被删除；无拒绝 → 通过 |
| 单关闭 / 双关闭 / 连续切换 / 重新启用等待 reload | Native | 同上：双关闭结束连接；重新打开后 `needs-reload` 逐个 Pi 消失；快速连续切换 | 通过 |
| 在途 frame（真实 Pi） | Native | `on_the_way`：手改 Pi 设置文件里 Deck 的条目（Deck 不被通知），于是 Deck 在提交下一帧时才得知，而那一帧正是不再允许的：run 的结束（signal）、session 的 teardown（lifecycle） | 测试探针记录该帧被回答 `revoked`；卡片上的 word 被撤回且**从未**写成 `turn-done`；teardown 从未写入；lease 保留、另一能力继续；另一个空闲 stream 在下一次心跳被告知 → 通过（两种运行时各两种帧） |
| 在途 frame（确定性） | Unit | `transport::tests::a_stream_hears_what_it_may_still_do_and_goes_on`；`pi-observer.test.mjs` “…a frame answered `revoked` is answered” | 通过 |
| Deck 自己的开关不产生在途帧 | Native | `capabilities` 每轮末尾断言探针的 `revoked` 列表没有增长（含“以较少能力绑定的新连接”） | 通过 |
| 能力少于声明的连接先被告知 | Unit | `transport::tests::a_stream_bound_with_less_than_it_claims_hears_so_before_its_acceptance`；`pi-observer.test.mjs` “bound with less than this runtime loaded: told before the acceptance, and nothing of the rest is sent” | 通过 |
| 未授予的能力伪造仍拒绝 | Unit | `transport::tests::session_frames_need_their_own_capability_and_are_gone_with_the_lease` | `capability-disabled` → 通过 |
| `pi-session` 之后“关 lifecycle 后继续完成一轮 Signal”的生产断言 | WK | `pi-live` 的 `pi-narrowed`；操作员核对 app.log：收紧与下一次 `turn-done` 之间没有 `observer bound`、没有 `observer ended` | 同一 stream 报告了下一轮 → 通过（npm、standalone） |
| UI current / needs-reload 与 runtime 一致 | Native + WK + Layout | `capabilities`、`disable`；`pi-live` 的 `pi-reload`、`pi-session`；`pi-layout` 的 Settings 各状态 | 通过 |

**剩余风险。** 通过 Deck 开关的收紧是即时的；用户手改 Pi 设置文件的收紧在该 stream 的下一帧生效（最迟下一次心跳，5 秒）。

### F04 完成真实可见 UI 门禁（中）

分两条，互不冒充：

- **Layout（脚本化应答）**：WKWebView 模式 `pi-layout`。生产前端模块在真实窗口里绘制真实的 Settings 对话框、真实卡片、真实 Needs attention 视图、真实“Why?”对话框和真实 Restart Review；**后端的回答由页面脚本给出**（`app/ui/test/pi-layout-smoke.mjs` 头部列出被脚本化的命令），没有任何 Pi、Claude 或 Codex 在运行。它证明“某个状态读起来是什么样”，不证明后端到达过那个状态。混合 Claude/Codex 的画面属于这一条，用的是 fixture。
- **Production WK（真实后端 + 真实 Pi）**：`pi-live`，npm 与 standalone 各一次。

覆盖（1280×800；中英文；50%、100%、160% 三档字号）：

| 门禁 | Layout（`pi-layout`） | Production WK（`pi-live`） |
| --- | --- | --- |
| G06 布局 | Board（长标题；Pi/Claude/Codex/shell 并列）6/6；Needs attention 6/6；“Why?” 36/36（每种 coverage × session 词）；Settings 48/48（条目的每种状态）+ 展开的说明 6/6 + 被拒绝的开关 6/6；Review mixed 6/6、noCandidate 6/6、failure 12/12（未设置观察、预览不可读）、结果 6/6 | 同一窗口尺寸下的真实 Settings、卡片、“Why?”、Needs attention、Review |
| C03 Review 截图与请求检查 | Pi 行无复选框、带 Pi 自己的原因；全选并选择 bypass 策略后没有任何 Pi 被选中或计数；发出的请求只含两个 Claude 卡片 | 真实后端的 preview：这张卡片被命名为 Pi，原因准确、不可勾选、唯一操作是普通重启；打开后不确认即关闭（页面在其余生里拒绝那条会重启 shell 服务的命令） |
| B12 lifecycle 开关与可用性投影 | 两个开关及其说明在每种状态下的位置与文案 | 真实开关（含确认）、`needs-reload`、卡片 payload 的 `observed`/`unsupported`、“Why?”文案；ID/路径不出现在 UI |
| A26 卡片 / Needs attention / viewed | 一个 Pi 的未读结束与两个 Claude 行并列列出；只有 coverage 的 Pi 不在待处理列表里，只出现在“无有效状态”筛选下 | 真实 Pi 的未读结束在真实 Needs attention 视图中列出（由窗口自己的按钮打开），返回后仍未读 |

每个场景检查：没有横向溢出（`scrollWidth`）、在对话框/窗口之内、行不重叠且按阅读顺序排列、出口按钮上方没有遮挡（`elementFromPoint`）、文案与 i18n 表逐字一致，并且没有任何形如原生 session id、pid、摘要、路径或 token 的内容被绘制（`pilay-no-native`：300 段被绘制的文本，0 处命中；脚本化应答里特意带了这类字段）。截图：`pi-layout` 64 张、`pi-live` 每次 13 张，全部哈希在 `snapshots.json`；`shots/` 是其中一部分的缩小副本（1280 像素宽的 JPEG），原图留在私有运行根里。

真实 Pi 场景看到的内容（F04 第二段的要求）：coverage（卡片与“Why?”里的 `connected` 文案；关闭 Signal 后 production poll 给出 `pi_signal: unavailable`，它与 `incompatible` 的渲染文案在 Layout 一条里检查）、lifecycle 不可用的原因（“Why?”里的 `unsupported` 文案）、两个开关及其说明、Restart Review 里 Pi 不可选的原因。

**按范围 BLOCKED（不是普通失败）：**

- 没有账户的**真实 Claude 重入**与真实 Codex 和 Pi 并列运行：BLOCKED（需要登录态；不借用用户账户）。混合画面只在 Layout 里用 fixture 给出，已标注。
- **系统通知 / Dock 徽标的真实发布**：BLOCKED（载体是 ad-hoc 签名，不能发通知；不改 TCC、不装系统服务）。通知的决定逻辑由既有单元测试覆盖，native 的 `viewed` 场景覆盖 episode 的已读规则。

**渲染中发现并已修复的问题：** Settings 打开时切换语言，Pi 的说明不重绘（`settings.js`）；`error.pi.neverRan` 由“目录不存在”推断“这台 Mac 没运行过 Pi”，已改为只陈述事实（负责人验收意见中的文案要求）。

**发现但未处理的既有问题（不属于 Pi，超出本轮批准范围，见 §9）。**

### F05 让证据对应可复核的最终构建（中）

- `scripts/pi_build_identity.py`：commit、工作区是否有差异及文件名、`git diff HEAD` 的哈希、对源文件内容的一个总哈希（干净检出与“字节相同的工作区”得到同一个值）。native 驱动与真实窗口操作员都把它写进结果；操作员拒绝一个比最新源文件还旧的 debug 可执行文件。
- 每份结果都带：实际命令、exit、逐项检查结果、测试可执行文件或载体内每个文件的哈希、observer 四文件哈希、运行时版本与哈希、清理结果。
- 上一轮证据的更正（`a18aa8a`）：`docs/evidence/pi-implementation-20261012/README.md` 里 A12“synthetic exec PASS”与 B02“静默丢弃 PASS”两行已就地更正，并在顶部注明那份文件的“final tree”没有可复核的构建身份。
- 没有“构建后改资源再复用旧结果”：debug 主程序与测试可执行文件都构建于被测提交里所有会被构建进去的源文件之后（操作员每次运行检查并记录 `built_after_every_source: true`；这项检查现在也覆盖被嵌入的 smoke 页面），全部证据在被测提交上重新生成。
- 分层报告见 §4，未运行与阻塞项见 §10。

### F06 共享 listener 的首行准入必须有界（高，基线缺陷）

这是 `1bdcf57` 已有的缺陷，不是 Pi 新引入的回归；Pi 的传输层上限在它之后才生效，所以本轮必须补上。

**根因。** accept 循环同步读取首行；2 秒是每次 `read` 的超时而不是整条首行的期限；连接逐个串行处理。一个每 0.5 秒发一个字节的连接可以无限期占住所有协议。

**修复**（`34082fd`，`agent_status.rs` 的 listener 一节）：

- accept 循环不再读任何字节。每个连接的首行由应用既有 async runtime 上的一个任务读取，**绝对期限 2 秒，从 accept 起算**（到达一个字节不会重置任何东西）。
- 同时处于“读首行”的连接最多 32 个（`MAX_FIRST_LINES`）；第 33 个立即关闭、不读、不排队（`listener-busy`）。首行最长 8 KiB（`first-line-oversize`）。
- 读完的连接经一个容量 32 的有界队列交给**一个**固定的 dispatcher 线程；队列满也是立即拒绝（`listener-busy`）。内存上界是 32 个各不超过 9 KiB 的缓冲加 32 个待分发的连接。没有新的监听 socket、没有每连接线程、没有无界任务或缓冲。
- dispatcher 就是原来 accept 循环里读完之后的那部分，三类协议的顺序与归属不变：先 Pi family，再 lifecycle family，最后 hook。Claude/Codex 的 hook helper 仍然是“连接还在时读取内核事实，然后才释放”；lifecycle receipt 的连接仍保持到 receipt worker 结算，其 deadline 从首行读完的那一刻起算。
- 首行之后已读到的字节不再被悄悄丢弃：对 Pi stream，它们是这个 stream 的后续字节，按同样的行/队列上限解析；对两个单消息协议，它们意味着对方说的不是这个协议，整条消息被拒绝（`trailing-bytes`）。期限到达时仍未结束的首行不解析（`first-line-late`）。Pi 正式 observer 仍然等 `accepted` 才发帧。

**测试**（真实 socket，production 的 accept、首行读取、dispatcher、receipt worker 与 Pi service；`agent_status::tests`）：

| 情形 | 测试 | 时间上界与实测 |
| --- | --- | --- |
| silent、partial line、drip（每 250 ms 一个字节）三个连接同时挂着 | `a_silent_or_dripping_client_holds_nobody_up_and_runs_out_of_its_own_time` | 期间 Pi hello、Claude hook、Codex hook、lifecycle receipt 各自在 1000 ms 内被分发并得到处理；三个慢连接各在自己的期限（≥1.95 s、<3.5 s）被关闭并计为 `first-line-late`；未结束的行没有被解析 → 通过 |
| 超长首行；尾随字节；EOF 结束的行；Pi hello 后紧跟的帧；空首行 | `a_first_line_is_bounded_and_what_follows_it_is_never_thrown_away` | 通过 |
| 连接数满 | `a_bounded_number_of_first_lines_is_read_and_the_next_connection_is_closed_at_once` | 第 33 个在 1000 ms 内被关闭；期限过后名额恢复，新的 Pi hello 在 1000 ms 内得到回答 → 通过 |
| 分发队列满 | `a_full_dispatch_queue_refuses_the_next_connection_instead_of_growing` | 队列恰为 32，之后的连接被拒绝 → 通过 |
| remainder 交给 stream parser | `agent_pi::transport::tests::what_was_read_behind_the_hello_are_the_stream_s_next_lines` | 通过 |
| 正常 Pi heartbeat / Signal | Native 全矩阵与 `pi-live`（都经过同一个 production listener） | 通过 |
| 负责人的滴流反例 | 等价于第一行的 drip 情形（真实 socket） | 探针不再等待慢连接 |

**剩余风险。** 满载时被拒绝的连接（包括 hook helper 的）就是丢了，只在 `listener-busy` 计数里留下痕迹——这是 Follow-up 批准的“有界拒绝，不宣称无限负载下必定接收”；同一用户的进程持续占满 32 个名额仍可让状态 socket 不可用（每个名额最多 2 秒），本轮不解决。dispatcher 是新增的一个固定线程（不是每连接线程）。hook 的提交顺序从“accept 的顺序”变为“首行读完的顺序”。

## 4. 分层覆盖

| 层 | 命令 | exit | 结果 | 未覆盖 |
| --- | --- | --- | --- | --- |
| Unit / Integration（Rust） | 门禁的 `cargo llvm-cov … -- --test-threads=1`（§5） | 0 | 整个 workspace 1502 项通过、0 失败、13 项 ignored；其中 `deck-app` 单元/集成 1228 通过；覆盖率 lines 85.66%、functions 83.60%（门槛各 75%） | 13 项既有 `#[ignore]`（需真实账户或由驱动启动的 harness） |
| Unit（前端与 observer） | `scripts/ui-tests` | 0 | 777/777 | — |
| Safety 扫描 | 同一次 `cargo llvm-cov` 内的 `edr_quiet`（20）、`external_admission`（8）、`log_privacy`（7）、`signal_census`（9）、`ipc_contract`（6）；`check-edr-binary`；`check.mjs` | 0 | 全部通过 | — |
| Native（真实 Pi，两种运行时，production listener/transport/store/poll） | `python3 scripts/pi_native.py --test-binary … --standalone …` | 0 | 269 项通过，0 失败；清理 exit 0 | 模型账户；真实窗口 |
| Production WK（真实窗口、真实后端、真实 Pi） | `python3 scripts/pi-live-verify.py --runtime npm --host <测试机>`；`… --runtime standalone --standalone … --host <测试机>` | 0 / 0 | 各 16/16 检查点、19 项操作员检查 | 签名发行包；通知发布；真实 Claude/Codex |
| Layout（真实窗口、脚本化应答） | `python3 scripts/pi-live-verify.py --mode pi-layout --host <测试机>` | 0 | 16/16 检查点，7 项操作员检查 | 不证明后端到达过这些状态 |

native 按场景（`native-scenarios.json`；最后一列是该场景期间 production listener 计数的闭合词，全部是场景自己有意造成的）：

| 场景 | 内容 | 通过 | 失败 | listener 计数 |
| --- | --- | --- | --- | --- |
| `settings_loop` | Settings 开关写入/取出 Deck 的唯一条目，逐字节 | 6 | 0 | — |
| `start_both` | 两种运行时的 hello 与 lease | 9 | 0 | — |
| `boundaries` | run 边界：普通、重试、排队、继续、settlement 启动、中止、provider 错误 | 22 | 0 | — |
| `viewed` | episode 的已读规则 | 3 | 0 | — |
| `dialogs` | Pi 的各类对话框不改变卡片（A18/A20） | 24 | 0 | — |
| `together` | 两个 Pi 同时运行 | 1 | 0 | — |
| `inactive_pane` | 不在前台的 pane | 4 | 0 | — |
| `reload` | reload 撤回旧 runtime 的观察 | 4 | 0 | — |
| `frozen` | 15 秒无帧：观察失效，不编造结束 | 5 | 0 | `pi-stale` ×2 |
| `restart` | Deck 重启 | 4 | 0 | — |
| `disable` | 运行中关闭开关；重新打开等待 reload | 8 | 0 | `pi-capability-disabled` ×2 |
| `not_loaded` | 未加载 / 被用户禁用 / 条目损坏 | 12 | 0 | — |
| `out_of_order` | 上游对话框事件乱序（A19） | 6 | 0 | — |
| `endings` | quit、被杀、普通 node 不是 Pi | 5 | 0 | — |
| `native_sessions` | P2：会话观察（含 standalone 的进程内 new/fork/resume/reload） | 33 | 0 | `pi-capability-disabled` ×1 |
| `capabilities` | F03：两个开关各自撤销，两轮、运行时互换 | 58 | 0 | `pi-capability-disabled` ×8 |
| `on_the_way` | F03：真实在途帧被回答 `revoked` | 25 | 0 | 无拒绝；`revoked` ×4（signal ×2、lifecycle ×2） |
| `unsupported_ids` | F02：256/257/512/513 字节的 session id | 29 | 0 | — |
| `exec_peer` | F01：真实同 PID exec、静默 exec、文件替换；外加三个后端生命周期的词表检查 | 11 | 0 | `pi-peer-executed` ×2 |

真实 Pi 在整个矩阵里没有一次 `pi-peer-executed` 或 `pi-no-instance` 拒绝；仅有的 2 次 `pi-peer-executed` 出自 `exec_peer` 的 C 程序。

native 每项检查见 `native-result.json`（带 AC 编号、名称、结果与细节）；真实窗口的检查点与操作员检查见 `wk-*.json`、`wk-*.verdict.txt` 与 `wk-*.applog.txt`（app.log 中与 Pi 相关的闭合行）。

## 5. 门禁

`gates.json` 是在被测提交上按 `.github/workflows/gate.yml` 的顺序逐步执行的记录（每步的命令、exit，以及测试数与覆盖率）：30 步全部 exit 0，开始与结束时工作区都干净、commit 都是 `eb16c71`（2026-10-12 04:51:29Z – 05:05:36Z）。

| job | 步骤 | 结果 |
| --- | --- | --- |
| static | 内置 tmux 的 SHA-256、Bergamot 补丁、`cargo fmt --check`、JS 语法、`scripts/ui-tests`、三个 Swift bridge 测试（speech、notification、smoke guard：5 个变异体全部被抓住）、`check.mjs`、Python 工具测试、`check-workflows`、`cargo audit`、tunnel-helper 的 fmt/clippy/test/audit | 全部 0；ui-tests 777/777；Python 177 项 OK；tunnel 28 通过；两次 audit 无漏洞（应用锁文件有 8 条既有的 allowed warning） |
| rust | `cargo clippy --workspace --all-targets --all-features -D warnings`、CE 探针环境、`cargo llvm-cov … --fail-under-lines 75 --fail-under-functions 75 … -- --test-threads=1`、coverage 的 sidecar provenance、CE Full Local parity 与 verdict | 全部 0；1502 通过 / 13 ignored；lines 85.66%、functions 83.60%；`full_local_parity:ci: pass` |
| release | `cargo build --release`、`check_local_translation_binary.py`、`check-edr-binary`、`package-compat-candidate` | 全部 0；EDR 词表干净 |
| after | 重新构建 debug 主程序与测试可执行文件、debug 的 sidecar provenance | 全部 0 |

关于最后一行：release 构建会覆盖各构建上下文共用的 sidecar 与前端产物，所以门禁结束时重新做了一次 debug 构建。重建出的 debug 主程序与测试可执行文件的哈希与被测的那两个文件**不同**（本项目的 debug 构建不是逐字节可复现的）。§1 与各结果文件里的哈希是实际运行过的文件的；这两个文件的原件仍在私有运行根里（native 根里的 `home/Applications/deck.app/Contents/MacOS/deck-app`，真实窗口根里的载体）。

适用性说明：

- `scripts/test-compat-candidate`（在 macOS 14 上经 LaunchServices 启动候选包）：**UNRUN**。脚本自己要求 macOS 14，本轮可用的两台机器都是 macOS 27；它的前一步 `scripts/package-compat-candidate`（打包，含 Pi observer 四个文件）已运行，exit 0。
- `release_channels.py gate-results` 是 CI 各 job 的汇总步骤，本地无对应物。
- 没有创建 candidate、tag、feed 或任何发布物；没有 push。

## 6. 清理审计

- **登记与停止。** 每次运行把自己启动的 tmux 服务与进程登记在该运行根的 experiment registry 里，结束时只经 registry 停止。最终四次运行的清理结果都是 exit 0、没有来自根的进程、没有遗留 socket（native 在开发机；三次真实窗口在测试机，由同一个操作员脚本在那里执行并记录在 `wk-*.json`）。本轮没有使用过按 PID 的 kill、`pkill` 或 `tmux -L`。
- **开发机。** 本轮被替代的 19 个运行根在核实后移入开发机的废纸篓（各自唯一命名，`<根名>-fu1-<时间戳>`），没有做不可逆删除：在本机运行过的根，registry `show` 确认没有存活的登记项；在测试机运行的根，其 registry 文件记录的是测试机上的 pid，在本机没有意义，所以不让 registry 对它动作（本机 `show` 对其中三个返回 3，“该 pid 现在是另一个进程”——已诊断，是 registry 按身份拒绝的预期行为），改为引用该次运行自己记录的清理结果；两类都要求本机没有任何进程的可执行文件位于该根之下。
- **保留。** 最终四个运行根（§11，私有日志）；`/private/tmp/deck-pi-xpgqUm`（上一轮下载并校验的官方 standalone Pi，两轮运行都用它）；上一轮证据的三个根（`deck-pi-ccmdhw54`、`deck-pi-live-13tv70bv`、`deck-pi-live-a1_wa3mf`）与 `deck-pi-iJiyNU`。负责人的根与文件（`deck-pi-accept-n_kowcmb`、`deck-pi-3sql7ynm`、`deck-pi-hm_25dtd`、`deck-pi-lufj0f2j`、`deck-pi-7p3p76lw`、`deck-pi-sjgfjj3z`、`deck-pi-review-ui.log`）没有被修改或移动。
- **测试机。** 运行前后各清点一次（不输出其主机与账户信息）：已安装的 Deck 与它的 tmux 服务前后都在运行（各 1 个进程），每次运行自己的检查也确认“同样的进程、数据目录里同样的文件名”；`/private/tmp` 下没有遗留的运行根；没有任何进程来自运行根；控制台未锁定。本轮 12 个运行根在测试机的废纸篓里（连同上一轮的 12 个，共 24 个 `deck-pi-live-*`）；执行者没有清空或整理任何废纸篓。
- **未触碰。** 没有修改真实的 Pi 或 Deck 配置与数据（每次运行都在自己的 HOME 下）；没有安装任何全局工具；没有改动系统安全设置、TCC 或 launchd；没有 push、merge、tag、release 或任何发布。

## 7. 原始失败与设施修复（单列，不计入产品测试结果）

本轮过程中出现过的失败，全部是执行者自己的测试期望或设施问题，逐条如实列出：

1. 并行 `cargo test` 下 `an_observation_reaches_no_mailbox…` 偶发失败（“a signed test file: Changed”），单独与串行运行均通过；门禁按 CI 形式串行运行。
2. `procinfo` 的真实 exec 测试最初复制 `/bin/bash`（arm64e 不能从别的路径运行），改用根内编译的 C fixture。
3. 真实 exec 的集成测试最初用 zsh 的 `zsocket` 当 peer，被 `pi-not-foreground` 正确拒绝（前台是 shell），改用 `scripts/pi/exec-peer.c`。
4. 两个源码扫描拦下了执行者自己的写法：`tests/edr_quiet.rs`（注释里出现了被禁止的调用名）；`tests/signal_census.rs`（字段名 `granted` 撞上保留的授权词，改名 `usable`）。
5. `node --test pi-observer.test.mjs` 两次挂住（失败的测试留下未关闭的 server），停止后改为带时限运行。
6. native 首次运行 80/81：`exec_peer` 的一个期望写错（把静默 exec 结束 lease 时的那次拒绝也算了进去）。
7. `pi-layout` 首次运行 5 个场景失败、第二次 1 个，全部是页面的期望有误（一个脚本化字段其实是会显示的 Deck 字段；160% 字号下标签在开关上方；同语言的 `setLocale` 会重置按钮文案；结果行本来就有“Open card”按钮；列表外的卡片没有先滚动到可见处）；第三次起通过。
8. `pi-live`（npm）首次运行所有检查点通过，但截图数的期望写错（实际 12，写成 14）。
9. 本次补充提交过程中：一处局部变量名遮蔽了同名辅助函数（编译错误）；一个旧测试钉住了旧的行顺序（已按新顺序改写）；页面里两处重复声明；一次 `cargo fmt` 空行；两个提交最初漏了标题后的空行，在任何 push 之前重新提交（`23292b2`/`887ff55` → `4427630`/`ef37976`，内容相同）。
10. `ef37976` 上的一轮完整运行（native 269/269；`pi-layout`、`pi-live` npm 与 standalone 均通过）没有被采用为证据，原因有二：期间执行者把证据暂存目录建在了仓库里，使其中一次 `pi-live`（npm）把工作区记录为 `dirty`（唯一条目是 `docs/evidence/pi-followup-01-20261012/`，`source_sha256` 与干净提交相同；移走后重跑通过）；随后自查发现载体清单的问题，需要再提交一次（`eb16c71`）。
11. `eb16c71` 的第一版里，新加的载体比对用了过宽的文件名匹配（`deck-mcp-*` 同时匹配 `deck-mcp-runner-*`），三次真实窗口运行都在本机准备阶段就停下（没有任何东西被启动或送到测试机，清理 exit 0）。修正匹配后重新提交并重跑。
12. 上一条发生时，一次 native 运行已经在旧提交上开始。执行者通过 experiment registry（`edr_runtime.py experiment cleanup --root …`）停止了它登记的 tmux 服务与进程（exit 0）。驱动随后自己的清理返回过一次 3：registry 拒绝了它，因为它的一个子进程已被停止但尚未被父进程回收（`<defunct>`，身份与登记不符）；驱动退出后再查，`show` 全部为 `gone`、再次 `cleanup` exit 0、没有来自该根的进程或 socket。没有使用任何按 PID 的 kill、`pkill` 或 `tmux -L`。
13. 本地门禁脚本的第二版把 `PATH` 写死，漏掉了 `cmake` 所在的目录：`eb16c71` 上的第一次门禁里所有需要构建的步骤都因“CMake is required to build vendored Bergamot”失败（没有任何东西被重新构建，已有的可执行文件哈希不变）。改为沿用调用者的 `PATH` 后整轮重跑，§5 是重跑的结果。
14. 清理脚本的第一版用“命令行里出现该根路径”来判断是否还有进程，于是把它自己的命令行算了进去，19 个根全部原地保留、什么也没移动。改为“可执行文件位于该根之下”后再执行。

## 8. 需要负责人裁决的设计点与偏差（执行者不自批）

1. **连续性事实的选择**：kqueue `NOTE_EXEC`（pidversion 仅作为实验佐证）。以及 §3 F01 的五条残余。
2. **exec 之后的识别**：被遗忘（之后只按名字 hold）；内核不可问时识别保留、coverage 为 unavailable。
3. **新增的 wire 元素**：`unavailable` 帧；服务端的 `capabilities` 与 `revoked` 行；`capabilities` 可以先于 `accepted`。observer 版本号仍是 1（Pi 集成尚未随任何版本发布过，没有旧 observer 需要兼容）。
4. **hello 声明多于设置时**：以交集接受，而不是拒绝。
5. **`needs-reload` 的含义扩大**：也覆盖“observer 已因能力被结束、Pi 仍在运行”。
6. **listener**：新增四个闭合的 drop reason、一个固定的 dispatcher 线程；期限到达时的不完整首行不再解析；单消息协议拒绝尾随字节；hook 的提交顺序见 §3 F06。
7. **UI 文案**：`settings.pi.note.needsReload`、`error.pi.neverRan` 的中英文改写，新增 `signal.pi.sessionUnsupported`。
8. **测试专用探针**：`transport::probe` 增加 `revoked` 与每个 lease 的可用能力（仅 `cfg(test)`），供 native harness 使用。

## 9. 发现但未处理的既有问题（不属于 Pi，未在批准范围内）

- 最小窗口（1280×800，默认四个分组）下，带 attention 徽标的卡片其标题被挤到没有宽度，卡片顶行横向溢出：`shots/pilay-board-en-100.jpg` 里两张 Claude 卡片只见徽标、不见标题（标题仍在侧栏里）。`pi-layout` 用 `pilay-m-card` 度量了它（仅度量，不参与判定；数值见 `wk-layout.applog.txt`：英文 100% 与两种语言的 160% 下各有 2 张卡片溢出）。这是卡片自身的既有样式，与 Pi 的行无关。
- 窄卡片上 Pi/Codex 的状态文字被省略号截断（链接“Why?”完整可达）。
- 同语言的 `setLocale` 会把重启按钮的文案重置为静态文本；Restart Review 的开场段落在切换语言后不重绘（页面因此按语言各打开一次）。

## 10. 未运行与阻塞

| 项 | 状态 | 原因 |
| --- | --- | --- |
| 完整 P1（needs-input / clear） | BLOCKED | 上游：Pi 1.1.0 的 UI 事件没有可靠顺序与等待快照 |
| P3 重入 | BLOCKED | 无法证明 Pi 供应方的运行代码身份；本轮未尝试放宽 |
| 真实 Claude 重入、真实 Codex 与 Pi 并列 | BLOCKED | 需要账户登录；不借用用户账户 |
| 系统通知 / Dock 徽标的真实发布 | BLOCKED | ad-hoc 载体不能发通知；不改 TCC |
| 签名 / 公证的发行包上的同样流程 | UNRUN | 本轮无发布授权，未构建发行载体 |
| macOS 14 候选包启动（`test-compat-candidate`） | UNRUN | 需要 macOS 14 |
| 其他 macOS 版本、Intel | UNRUN | 两台机器都是 macOS 27.0.1 arm64 |
| Pi 1.1.0 以外的版本 | UNRUN | 只有 `native-result.json` 列出的两个制品 |

## 11. 本目录的文件

| 文件 | 内容 |
| --- | --- |
| `gates.json` | 门禁逐步记录 |
| `native-result.json`、`native-scenarios.json` | native 运行的摘要：构建身份、制品、每项检查；按场景的检查数、耗时与 listener 计数。不含任何原生 session id |
| `wk-layout.json`、`wk-live-npm.json`、`wk-live-standalone.json` | 真实窗口三次运行的摘要：构建身份、载体清单、运行时、检查、截图哈希、清理 |
| `wk-*.verdict.txt`、`wk-*.applog.txt` | `scripts/smoke-verdict` 的结论；app.log 中的检查点与 Pi 相关闭合行 |
| `snapshots.json`、`shots/` | 三次真实窗口运行全部截图的 SHA-256；其中 32 张的缩小副本（`shots/index.json` 给出每张对应的原图及其哈希；`npm-`、`standalone-` 前缀表示出自哪次 `pi-live`，无前缀的出自 `pi-layout`） |
| `exec-primitive/` | F01 原语实验的程序与原始记录 |

私有运行根（含完整日志与原图，日志里有原生 session id，不提交）：native `/private/tmp/deck-pi-zxpp_mb6`；`pi-layout` `/private/tmp/deck-pi-live-shxag3xf`；`pi-live` npm `/private/tmp/deck-pi-live-3ra6o90z`、standalone `/private/tmp/deck-pi-live-nri6m51z`（真实窗口的三个根在开发机上保留了取回的日志、app.log 与截图；它们在测试机上的原件已移入测试机的废纸篓）。
