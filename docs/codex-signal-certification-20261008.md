# Codex 0.161.0 / Deck Signal Integrity 复认证（2026-10-08）

以下记录的是 2026-10-08 的调查、认证和无内容证据；当日未提交。2026-10-10 仅整理为独立文档提交，没有启动新实验。原始证据保留于本地 ignored 报告目录；提交中的 session/turn ID 是一致匿名标签，路径占位符不是原始值。详见[证据发布说明](evidence/codex-signal-20261008/README.md)。

原调查仅调查、认证和保存无内容证据。Deck 产品代码、用户配置、hooks、scheduler/readiness 策略均未修改；无 commit、push、版本变更或发布。边界：**Deck owns attention around work. Codex owns the work.**

## 1. Exact version / stable release / source revision

| 项目 | 本次精确对象 |
| --- | --- |
| 调查日期 | 2026-10-08，Asia/Tokyo；保留的运行采样为 06:17:02.926–06:19:43.978 UTC |
| 本机 CLI | `codex-cli 0.161.0` |
| 实际二进制 | Homebrew Cask 安装的 0.161.0 `bin/codex`；精确文件摘要见下行，发布版不保留本机安装路径 |
| 调查当日官方稳定版 | [rust-v0.161.0 release](https://github.com/openai/codex/releases/tag/rust-v0.161.0)，当日查询官方 GitHub `releases/latest`，draft=false、prerelease=false |
| 发布时间 | 2026-10-07T15:58:45Z |
| annotated tag object | `7e21416b38834816c224ea0dfd135c3de94b2f15` |
| peeled commit / 实际 checkout HEAD | `979011409de0a60b52f179721948e65531d26144` |
| 比较基线 | 0.160.0，`a956835d020762cb2b570053af06f643a11c0ecc` |
| 实验版本一致性 | 两个 shared TUI、embedded TUI、临时 home 内 daemon 二进制均为 0.161.0；daemon 副本与本机 CLI 的 SHA256 相同 |
| CLI / daemon SHA256 | `12ac11d2c7eee27cfae34393986d7b7c9ed0dea537cb749831cdd7033893e6de` |
| Deck 审查对象 | HEAD `7ad6fe0f2b0946ddcac657f4cbb36f2e587ba91e`；初始 worktree 干净 |

这证明版本及本次使用的二进制一致，不声称从源码完成了可复现构建或独立验证官方构建证明。没有升级或重装本机 Codex。所有下列上游源码链接固定到完整 commit，不能用浮动 main 替代。

源码默认值与实验中 `features list` 的实际值一致：[feature 定义][features]、[attention 相关定义][attention-features]。

| Feature | 源码 stage / 默认值 | 隔离实验实际值 |
| --- | --- | --- |
| [daemon_auto_start](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L967) | stable / true | true；另以 socket、PID、ancestry 验证真实共享 |
| [hooks](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1254) | stable / true | true；六个定义经正常 TUI trust 审查 |
| [auth_elicitation](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1816) | stable / true | true；没有配置或调用真实外部 MCP |
| [guardian_approval](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1707) | stable / true | true；存在这个 flag 不代表每个批准交给自动 reviewer |
| [tool_call_mcp_elicitation](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1809) | stable / true | true |
| [write_stdin_approval](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1247) | stable / true | true |
| [default_mode_request_user_input](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1683) | under development / false | false |
| [exec_permission_approvals](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1241) | under development / false | false |
| [mcp_oauth_refresh_coordination](https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1443) | under development / false | false |

实验配置关闭 plugins、plugin_hooks、memories、multi_agent、analytics 和通知，使用 file credential store、read-only sandbox、on-request approval。daemon updater 仅在临时状态中关闭。shared TUI 无 `-c`、`--enable/--disable`、trust bypass 或 remote override；控制组只有明确的 `--no-daemon`。配置不存在 MCP servers，也没有复制用户 skills/plugins/config。

## 2. Executive verdict

**VERDICT C — STILL FUNDAMENTALLY BLOCKED（shared-daemon attribution）。**

0.161.0 实际复现 0.160.0 的核心反例：B 的 hook 仍继承 A 的 pane/socket marker，内核 ancestry 为 `hook → daemon → A TUI → A pane shell`，无法到达 B 当前 foreground。指向 A 时，hook 与 daemon 的两个 terminal-less process group 又违反 Deck 的严格 terminal continuity。

18 个 shared 采样均无法通过当前归属模型；5 个 embedded 采样的自身 pane/foreground/terminal continuity 均通过独立算法核对。这是进程归属认证，**不是安装版 Deck ingest、通知、scheduler 或完整 attention coverage 认证**。

新版存在 MCP OAuth login correlation 等协议改进，下面如实列出；它没有补上 client→current pane generation 的证明。因此不能解除 shared-daemon 限制。状态质量和归属可信度必须分开评价。

## 3. 相比 0.160.0 的实际源码变化

逐字节比较结果保存在 [identity.json](evidence/codex-signal-20261008/identity.json)，包括双方 SHA256。

| 实际检查 | Diff 与意义 |
| --- | --- |
| hooks/src/schema.rs | **byte-identical**：没有新增 attention/client/PID/tty command-input 字段 |
| hooks/src/registry.rs | **byte-identical**：宿主 std::env 快照及 reconfigure 保留逻辑不变 |
| hooks/src/engine/command_runner.rs | **byte-identical**：环境回放及 NewSession spawn 不变 |
| app-server-transport/src/transport/unix_socket.rs | **byte-identical**：控制 socket 和 transport 没有 per-client hook bridge |
| protocol/v1.rs、v2/thread_data.rs | **byte-identical**：ClientInfo、Thread 身份结构不变 |
| hooks discovery / dispatcher / matcher | matcher 编译、复用及调用类型重构；没有 client ownership 数据流。[matcher][matcher]、[dispatcher][dispatcher] |
| app-server thread_state.rs | 新增 `goal_resume_lock`，序列化 goal 修改与 cold/path-based resume；不是 TUI ownership lock。[goal resume lock][goal-lock] |
| app-server v2/mcp.rs | OAuth login response/completed notification 增加 optional `loginId`；当前响应总返回，兼容旧服务器缺省；它关联一次登录尝试。[MCP OAuth][mcp-oauth] |
| app-server v2/thread.rs | goal set/clear 增加 user/automatic mutation origin；这是操作来源分类，不是 OS client provenance。[goal origin][goal-origin] |
| app-server v2/thread_prediction.rs | 新增 thread prediction notification；没有 client PID/tty binding。[prediction][prediction] |
| core session | 新增 host-supplied `turn_extension_init` 传播、resume/persistence 等调整；没有传播到 command-hook input 或 pane proof。[session configuration][session-config] |

旧认证仅作为对照；本次结论来自上述固定 release 源码和新运行证据。没有因为 spawn API、setsid 或名称变化推断修复。

## 4. Hook schema diff

六类 command-hook input 无变化。以下字段均为 string，除特别标注。`R`=required、不可 null；`R?`=required 但值可 null；`O`=optional，缺失时不序列化，当前 schema 不接受 null。`NullableString` 的 null 规则见[定义][nullable]。

六个 generated JSON schema 也与 0.160.0 逐字节相同，required/properties/null 定义见 [schema-summary.json](evidence/codex-signal-20261008/schema-summary.json)。

公共字段 `C`：`session_id(R), transcript_path(R?), cwd(R), hook_event_name(R)`。
公共字段 `M`：`model(R), permission_mode(R)`。

| Event | 精确输入字段 | optional / nullable 说明 |
| --- | --- | --- |
| SessionStart | C + M + `source(R)` | 仅 transcript_path 可 null；source 枚举 startup/resume/clear/compact/fork。[schema][session-schema] |
| SessionEnd | C + `reason(R)` | reason 当前固定 `other`；没有 turn_id、model、permission_mode。[schema][session-schema] |
| UserPromptSubmit | C + M + `turn_id(R), prompt(R), agent_id(O), agent_type(O)` | agent 字段只在适用 subagent context 出现；prompt 不得作为 Deck 证据保留。[schema][prompt-schema] |
| PermissionRequest | C + M + `turn_id(R), tool_name(R), tool_input(R), agent_id(O), agent_type(O)` | tool_input 是任意 JSON Value，值可包括 null；字段本身 required。没有 tool_use_id、client_id。[schema][permission-schema] |
| Stop | C + M + `turn_id(R), stop_hook_active(R bool), last_assistant_message(R?)` | transcript 和 assistant message 可 null；后者不应被 Deck 读取或保留。[schema][stop-schema] |
| Interrupt | C + M + `turn_id(R)` | 无 optional 字段；仅 transcript 可 null。[schema][interrupt-schema] |

`hook_event_name` 对每个结构是相应常量；permission_mode 枚举为 default/acceptEdits/plan/dontAsk/bypassPermissions。[枚举约束][schema-enums]。结构使用 deny_unknown_fields，不存在隐藏的 command-input client metadata 字段。

完整公开 hook 事件为 PreToolUse、PermissionRequest、PostToolUse、PreCompact、PostCompact、SessionStart、SessionEnd、UserPromptSubmit、SubagentStart、SubagentStop、Stop、Interrupt。[HookEventName][hook-events]。**没有新增 ordinary request_user_input、Plan/choice、MCP elicitation、authentication/login attention lifecycle command hook**。协议中的 server requests 和 notifications 不等于 command hooks。

身份语义：

- `session_id` 来自 `sess.session_id()`，是 root thread 与其 descendant threads 共用的 session tree 身份；root 时等于 root ThreadId。不能把它普遍解释成当前 concrete thread ID。resume 恢复保存的 session 身份；非 root 使用 controller identity。[真实语义][session-identity]、[初始化][session-root]、[hook 构造][hook-build]。
- `Thread.id` 是 concrete conversation/thread UUIDv7；`Thread.sessionId` 是上述共享 session tree 身份，forkedFromId 和 parentThreadId 表示关系。它们不是终端身份。[Thread][thread-data]。
- `turn_id` 来自 runtime TurnContext.sub_id；submission ID 的 UUIDv7 由 core 生成，steer 可以加入现有 turn。它是 turn correlation，而非客户端实例。[submission ID][submission]、[turn/start][turn-start]。
- `agent_id/agent_type` 是 subagent 身份/类型，不是 interactive TUI。客户端提供的 `clientUserMessageId` 是用户消息 correlation；Responses metadata、originator 和 clientInfo 也不是 pane ownership。[turn params][turn-params]。
- 这些 command-hook input 没有 client_id、connection_id、subscriber_id、client/TUI PID、tty/terminal identity、transport peer identity 或 frontend instance token。公开 Thread/clientInfo 也不提供经过验证的进程/终端身份；内部 ConnectionId 的不同语义见第 7 节。允许 thread/session/turn ID 是为了关联事件，绝不能用 ID 的唯一性推导 pane 归属。

**是否出现足以绑定具体 interactive TUI 的字段？没有。** Runtime 生成的 ID 能表示自己的会话/回合，但客户端可请求 resume 已知 thread，多个 client 可订阅同一 thread；OS 没有验证这些 ID 属于哪个 pane。自报 clientInfo/originator 可由同用户客户端提交，验证格式或能力协商不构成 PID/tty attestation。断连撤销 subscription，不会撤销仍存在的 conversation ID，更没有可供 Deck 使用的 pane-generation binding 失效规则。

## 5. Hook environment dataflow

实际传播链：

`TUI initialize / thread start-or-resume / turn start → app-server connection routing → core Session + TurnContext → Hooks event serializer → dispatcher → CommandHookRuntime → command child → helper`。

1. **创建层级**：core session 初始化调用 `Hooks::new(config, thread_id, ...)`；registry 每个 session 创建 snapshot，但读取的是所在宿主进程的 `std::env::vars_os()`。shared 下宿主是 daemon，不是该 session 的 TUI；不是每 connection/turn/call 重新采集。[core init][hooks-init]、[registry][registry]。
2. **保存/刷新**：`Hooks.environment` 为 Arc；`Hooks::reconfigured` 和 `CommandHookRuntime::reconfigured` 克隆原 snapshot 和运行状态，不重新读取客户端环境。[registry][registry]、[runtime][command-runtime]。新 thread/resume 即使新建 runtime，仍从 daemon 宿主采集；没有 reconnect 刷新 per-client env 的代码路径。
3. **override 的边界**：普通 config/managed hook 的 `HookHandlerSource.env` 是空 map；插件有静态 source env（如 PLUGIN_ROOT/PLUGIN_DATA）。dispatcher 传 `handler.env`，runner 在 snapshot 之后叠加并过滤受限变量。没有由连接 peer 身份生成、验证、存储和随 foreground 更新的 pane override。[discovery][discovery]、[dispatch][dispatch-exec]。
4. **env_clear 与注入**：Unix 的 `codex_utils_pty::Command` 默认从空环境开始，非 Unix runner 显式 env_clear；随后回放 snapshot，再叠加 handler env，并过滤 NON_INHERITABLE_ENV_VARS。过滤名单含 Noise/工作负载身份 token 等，不含 TMUX_PANE、TERM、DECK_STATUS_SOCK。[runner][command-spawn]、[empty env][empty-env]、[受限名单][scrub-env]。
5. **具体环境来源**：TMUX/TMUX_PANE 是 pane 内启动 TUI 时的继承值；TERM 来自启动环境；DECK_STATUS_SOCK 在实验中是 A/B/E 各自启动命令设置的 harmless marker。daemon 从 A 启动，所以其 Session snapshot 都看到 A。tty 相关环境不是内核 controlling tty，不能相互替代。
6. **两个 shared TUI 能否得到不同且可信环境？** 本次实际得到相同 A marker；源码没有可信 per-client 环境。即使未来允许 B 自报一个正确 marker，也仍需独立 ownership proof。
7. **stale environment**：reconfigure 明确保留 snapshot；TUI disconnect/reconnect 不改 daemon 宿主 env；resume/new runtime 的宿主仍不变。这是源码结论，不声称本次完整跑过 reconnect/reload 矩阵。
8. **spawn 不补 provenance**：runner 使用 `ProcessMode::NewSession`，macOS 实现含 POSIX_SPAWN_SETSID。它只影响子进程 group/session/tty，不会将 B hook 重新挂到 B ancestry。[spawn][command-spawn]、[macOS implementation][posix-spawn]。

## 6. Shared-daemon process topology

Deck admission（以下项目源码位置按文档整理时的 abd325b 核对，历史审查对象仍是第 1 节所列版本）：[peer PID](../app/src-tauri/src/procinfo.rs#L183)、[同一快照 ancestry](../app/src-tauri/src/procinfo.rs#L210)、[terminal_continuous](../app/src-tauri/src/agent_origin.rs#L48)、[ingest](../app/src-tauri/src/agent_status.rs#L930)。要求 kernel peer PID、pane process ancestry、CURRENT foreground generation（PID+birth）、严格 terminal continuity 同时成立。

本次 B-only UserPromptSubmit 样本（数字均为实测；tty=0 表示无 controlling terminal）：

```text
B TUI 96587 — B tty 268435460，foreground PGID 96587
       RPC 到共享 daemon；不在 hook 的 ancestry 中

hook/helper 97046  PGID 97046  tty 0
  → daemon 96476   PGID 96476  tty 0
  → A TUI 96466    PGID 96466  tty 268435459
  → A shell 96461  PGID 96461  tty 268435459，foreground PGID 96466
  → isolated tmux 96460
```

- 对 B：pane process 96583、foreground 96587 均不在 chain，无法证明 B ownership。
- 对继承环境指定的 A：pane/foreground ancestry 成立；但 detached leading run 不是同一个 group，97046 与 96476 分属两个 terminal-less PGID，terminal continuity **失败**。
- 不存在本次路径上的 per-client child、官方 bridge 或 pane-local hook execution。B 的 PermissionRequest（97258）与 A Interrupt（97321）也保持同样断点。
- **回答核心问题：B hook 不能沿 ancestry 到达 B 当前前台 TUI。** RPC 连接不是 parent-child 关系；共享 daemon 的 ancestry 不代表其他连接客户端。

这不表示 Deck 应按 B 的 session ID 或时间去给 B 写 Unavailable。当前代码只在已经证明到达某 pane/current generation、随后 terminal-discontinuity 时降级该 generation；foreign-pane/generation-mismatch 不为其他 pane凭关联制造信任记录。

## 7. App-server provenance 与生命周期

| 机制 | 实际语义 / 认证判断 |
| --- | --- |
| initialize.clientInfo | name/title/version 是客户端提交的 app 标签。ConnectionSessionState 保留 client name/version、capabilities、transport origin；无 PID/tty。[v1][client-info]、[connection state][connection-state] |
| ConnectionId / origin | 服务器内部 AtomicU64 递增 connection ID，origin 区分 Stdio/InProcess/WebSocket/RemoteControl；不是 kernel peer PID。daemon 重启计数重新开始；没有对 Deck 暴露的 pane epoch。[transport][connection-id] |
| Thread subscriptions | `HashSet<ConnectionId>` 多对多；`first_attestation_capable_connection_for_thread` 按能力及较小 ID 选连接，不是唯一 owner/current operator。attestation 能力不是 pane provenance。[registry][thread-state] |
| start / resume / fork | 创建/加载/复用 thread，并将 connection 添加为 subscriber。running-thread resume 也添加连接；cold resume 可创建 runtime/应用配置，不能作为默认只读观察订阅操作。[subscription][subscription]、[running resume][running-resume] |
| Thread.id / sessionId | conversation / session tree；originator 是创建时记录的来源，与当前 client/executor 无关。没有 TUI PID 映射。[Thread][thread-data] |
| thread/loaded/list | 只返回已加载 thread ID 的分页快照，不含 owner、client PID 或 tty。[loaded list][loaded-list] |
| thread/read(includeTurns=false) | 可 metadata-only read；仍有 preview、cwd、path 等非 attention 内容，Deck 即使未来使用也应严格丢弃。没有订阅列表/pane mapping。[read][thread-read] |
| thread/status/changed | threadId + status，active flags 有 waitingOnApproval/waitingOnUserInput；语义比 spinner 强。该结构无序列号、epoch、pane-generation binding。[status schema][status-schema]、[status facts][status-facts] |
| disconnect / reconnect | remove_connection 撤销当前 subscription；thread 可继续存在，新连接重新加入。没有恢复 TUI↔pane binding 的公开只读快照。[remove connection][subscription] |
| daemon / observer / Deck restart | loaded-thread recovery 可恢复工作线程，不是 TUI/tty ownership。Deck 没有已建立的权威绑定可恢复；不能把 persisted thread ID 与新的进程 generation 自动续接。[recovery][daemon-recovery] |
| background terminal osPid | `ThreadBackgroundTerminal.osPid` 指 tool/background terminal 进程，不是 interactive TUI PID；还带 command/cwd，不能拿来当 pane owner。[background schema][background-schema] |
| environmentId / Thread.extra | 选择执行环境/workspace，独立于 connection；extra 当前为空结构。不是终端身份。[environment][environment-schema]、[extra][thread-extra] |

**Deck 能否不接管 Agent、不 start/resume thread、不解析聊天，将 threadId 可信绑定到既有 TUI/pane generation？当前没有发现满足条件的官方路径。** 元数据读取是有价值的只读状态快照，但不是所有权认证；普通订阅也不是受限 observer capability。没有验证过的 client PID、peer→TUI 映射、前台 generation 证明、stale-binding epoch/撤销协议，不能以 clientInfo、subscriber 或创建者代替 owner。

`/new` 应有新的 concrete thread；resume 可保持原 thread；fork 创建关系；同 thread 多 client 可同时订阅。源码的这些生命周期不提供 terminal ownership，不能唯一指定某张 Deck 卡。本次未执行这些扩展运行矩阵，也没有启动 headless observer/app-server orchestration。

## 8. Attention-event coverage matrix

下表把 **事实语义** 与 **归属** 分开：shared 下即使事件语义权威，也没有可信 pane binding。`源码` 不等于运行通过。

| 事实 | Command hooks | App-server / TUI | 本次运行与可信度界限 |
| --- | --- | --- | --- |
| 1. prompt accepted / turn working | UserPromptSubmit 表示进入提交处理阶段；后续 sync hook 可 stop/block；不能当最终 accepted | turn/started 为 runtime turn 已启动的权威事实，仍不保证 user prompt 已通过全部 hooks/工具策略。[input][input-inspect]、[turn event][turn-event] | A/B/embedded 有 submit；不把它升级为任务成功或发送 readiness |
| 2. permission approval needed | PermissionRequest 在 hook/reviewer resolution 之前，可由 hook allow/deny 或 Guardian 解决 | item command/file/permissions requestApproval 是真正送往客户端的请求；autoApprovalReview started/completed 是自动审查阶段。[precedence][approval-precedence]、[protocol][requests] | B 真正出现批准 UI，审查内存中的 exact `/usr/bin/true` 后批准；不能据此认为每个 PermissionRequest 都需人类 |
| 3. ordinary request_user_input | 无专用 hook | item/tool/requestUserInput + pending user-input guard；支持 blocking 与 auto-resolution 信息。[request handling][request-input] | 源码；default-mode feature=false，本次未运行。自然语言提问本身不是结构化 request |
| 4. Plan / choice UI | 无专用 hook | 使用 request_user_input 时有对应 server request；TUI 在 turn completion 后可调用 `maybe_prompt_plan_implementation` 并打开本地 selection view；该 overlay 没有独立 server attention lifecycle。[本地实现][plan-overlay] | 源码/未运行；title/action overlay 属 presentation，不补身份 |
| 5. MCP elicitation | 无专用 hook | mcpServer/elicitation/request；线程状态将此归入 pending permission guard，不一定 waitingOnUserInput。[MCP handling][mcp-elicitation] | 源码；未配置外部 MCP、未运行 |
| 6. authentication/login | 无专用 hook | account login start/complete/cancel；MCP OAuth loginId；auth_elicitation 将适用 connector auth 送入 URL elicitation。并非所有启动/login UI 都有一个统一 thread attention event。[auth][auth-elicitation]、[account][account-login]、[OAuth][mcp-oauth] | 源码；没有发起真实登录/撤销用户授权。loginId 只关联登录尝试 |
| 7. Stop | 是尝试结束：其他 Stop hook 可要求 continuation；async observer 可能先看到 Stop | turn/completed 更接近本次 runtime turn 的终止事实，可 completed/failed/interrupted；仍非用户任务完成或无后台活动。[continuation][stop-continuation]、[completion][turn-complete] | 正常回合 Stop 已运行；没有配置 continuation hook 或认证自动续行矩阵 |
| 8. Interrupt | active turn 中断的 hook | turn/completed interrupted / abort 语义；cleanup-background-terminals 是独立操作。[interrupt][interrupt-source] | A UserPromptSubmit→Interrupt 已运行；不能推断所有 background terminals 已停止 |
| 9. background terminal still running | 没有完整独立 hook | backgroundTerminals/list 提供结构化快照；TUI Waiting 是 presentation。idle/active 枚举不包含“无后台进程”证明。[background][background-schema]、[separate cleanup][background-clean] | 源码；本次没有认证后台活动完整矩阵 |
| 10. 真正到 terminal interaction boundary | Stop/Interrupt 不完整，也没有覆盖全部本地 UI | turn terminal event 是 runtime turn 边界；TUI 底部 overlay、queued input、local dialogs、reconnect 和后台工作仍可能存在 | 未发现可直接当作“当前 pane 已可安全输入”的全覆盖权威事件；未做 installed Deck 全链 |

SessionEnd 是 root thread/runtime teardown，reason=`other`（[root-only 调用限制][session-end-root]）；shared TUI disconnect 不必终止 runtime。不能将它当作每个 TUI 的退出 hook。[SessionEnd][session-end]、[shutdown][session-shutdown]。

**Stop、Ready、idle、无 spinner、静默都不表示任务成功、允许关闭卡片、无后台工作或自动输入安全。** 状态事件可 inform/HOLD，不产生执行授权。

## 9. 双客户端 runtime evidence

先确认源码隔离，再启动：

- `find_codex_home` canonicalize 显式 CODEX_HOME；控制 alias 和 daemon PID/settings/operation locks 归该临时 home。[home resolver][home-resolver]、[daemon paths][daemon-paths]。
- Unix listener 对 alias parent canonicalize 后的完整路径取 SHA256，实际绑定到 `/private/tmp/codex-daemon-<uid>/<hash>`。UID root 共用不等于共用一个 daemon；本次 hash/socket/lock 启动前均不存在，alias resolve 与预计 hash 完全匹配。[rendezvous][rendezvous]。
- `--no-daemon` 在 discovery/start 前排除共享路径；非允许 CLI overrides 也可能排除，故 shared 不使用 override。[launch policy][launch-policy]。
- 同一私有 HOME/CODEX_HOME、同一空 cwd、独立 tmux server/socket；两 pane `%0` / `%1` 的 tty、TUI foreground PID 不同。共享 CODEX_HOME 是双客户端实验设计的一部分，与用户 home 独立。
- A 启动 daemon PID 96476，B 启动后仍为该 PID；两客户端 hook 的共同 parent 96476，daemon 二进制 hash/version 与 CLI 一致。不是只看 flag 猜共享。
- Hooks 只安装在临时 home，六个相同已审查 helper 定义，通过启动 TUI “Trust all and continue”，未 bypass。`/hooks` 表格确认 installed=1 / active=1。
- helper 解析 stdin 后仅保留事件/允许的 ID，立即丢弃其余 payload；只取格式受限 TMUX_PANE 和 A/B/E marker。输出到固定私有 Unix socket，等待采样完成后退出。
- collector 用 macOS LOCAL_PEERPID 取 kernel PID，在 helper 连接存活时一次 libproc process-table snapshot 中关联全部 ancestry、pane 与 foreground PID/birth/tty/PGID。不保存 argv、环境、终端 capture、聊天、prompt、模型输出或 credentials。这里的 snapshot 是一次 PID 枚举后的逐项 libproc 读取，不是内核原子快照；helper 保持存活，记录并核对 PID+birth。仍未认证安装版的 race/losslessness 处理。

| 最低矩阵 | 观察结果 | attribution 结论 |
| --- | --- | --- |
| A only | UserPromptSubmit → Stop | marker A；A ancestry 成立；terminal continuity FAIL |
| B only | UserPromptSubmit → Stop，另有 SessionStart | marker **A**；B pane/foreground ancestry FAIL |
| A/B overlap | 双方各 submit/stop；两个 hook interval 重叠约 **24.33s** | ID 分离，但双方均 marker A / daemon ancestry；FAIL |
| 无操作 approval/request | B submit → PermissionRequest → Stop；正常批准 UI；仅批准 exact `/usr/bin/true` | 归属仍是 A；FAIL |
| interrupt | A submit → Interrupt | 双 terminal-less PGID；FAIL |
| 后续回合 | A/B 各 submit → Stop | session ID 各自稳定、新 turn ID；归属断点没有恢复 |
| embedded control | E 两回合 submit/stop，另有 SessionStart | 5/5 自身 pane/foreground ancestry 和 continuity PASS，详见下一节 |

保留 23 条无内容事件（shared 18，embedded 5）、10 个 turn、3 个 session tree ID。起止配对依据完整 `(session_id,turn_id)`，**不按阶段时间猜归属**。例如 approval 的 Stop 可能在 collector 已切到 interrupt phase 后到达，analysis 仍将其配回 approval turn。24.33s 是 hook-observed turn interval 重叠，不是模型 token 流或工具实际执行时长的测量。

证据：[events.jsonl](evidence/codex-signal-20261008/events.jsonl)、[analysis.json](evidence/codex-signal-20261008/analysis.json)、[features.txt](evidence/codex-signal-20261008/features.txt)。分析中的 A/B/E 只是实验标签，不能成为产品 pane 绑定依据。[离线核对脚本](evidence/codex-signal-20261008/verify-evidence.py) 重新计算同一严格 continuity 算法、核对 PID+birth 及证据 allowlist；exit=0。

如实记录 setup 限制：最初 `features list` exit=1，原因为 0.161.0 拒绝 `approval_policy="untrusted"`；改临时配置为 on-request 后 exit=0。最初 background collector 未保持存活，未取得可认证事件；随后改为持有前台 exec session 并重新跑矩阵。一个主动连接/立即断开 collector 的 liveness probe 触发 worker 的 ENOTCONN（57），不属于 hook 样本；后续保留样本均有 kernel peer 和完整 snapshot。这不是全运行 losslessness 认证，没有把缺失采样计为 pass。collector 最后以 Ctrl-C 正常终止，exec exit=130。

## 10. Embedded control evidence

同一隔离环境第三 pane `%2` 使用正常 interactive `codex --no-daemon`，没有启动 headless harness。

```text
E hook 97437   PGID 97437  tty 0
  → E TUI 97395 PGID 97395 tty 268435461，foreground PGID 97395
  → E shell 97389 PGID 97389 tty 268435461
  → isolated tmux 96460
```

SessionStart + 两个 UserPromptSubmit/Stop 合计 5 个 kernel-peer samples。各样本 marker E、pane `%2`；只有 hook 自己一个 detached group，其 group leader 直接到 E TUI；对 E 的 continuity 全部 PASS，不到达 A/B foreground。

结论仅为 **0.161.0 embedded attribution topology 可满足现有模型**。未运行 embedded permission/interrupt/background/full lifecycle/安装版 ingest 矩阵，不声称完整 Signal 或安装版 Deck 功能认证通过。不替用户改启动方式，不停止用户 daemon。

## 11. 当前 Deck trust policy 能否改变

**不能改变。** 保持 `CodexSignalTrust::Unavailable`、Unknown、现有 scheduler/readiness 与 first-interaction gates；本次没有修改实现。[scheduler contract](../app/src-tauri/src/scheduler/select.rs#L1)。

共享 hook 指向 A 且 ancestry 到达 A generation 时，terminal-discontinuity 的现行降级有证据；不得再通过 session/thread ID、cwd、时间或共同 daemon 给未证明的 B generation 强行写 Unavailable。B 没有正面证据时保持 Unknown/未建立。

安全拒收通过不等于 attention coverage 通过。状态缺失应保持诚实的低干扰诊断；title/cwd/transcript/thread-name guessing 最多是 presentation/degraded evidence，不得释放自动输入、关闭卡片或更改任务完成语义。

## 12. 有依据时的最小未来设计（只设计）

1. 优先官方 **结构化 client provenance**：服务器从本地 transport 取得/验证 kernel peer PID 和进程 birth，证明连接实际属于当前 interactive TUI；TUI 显式报告 active thread/turn 生命周期，服务器绑定 connection epoch。仅自报 PID/token 或 clientInfo 不够。
2. 或官方 **pane-local topology**：hook/bridge 由触发它的当前 TUI 直接执行，服务器事件交给该 TUI；实际 helper ancestry 与单 detached group 满足现行规则。必须认证 multi-client 同 thread 时哪些客户端应收到何种 attention，而非盲目广播/猜 owner。
3. 或受限 **observer/status API + authoritative TUI↔thread binding**：只读、无聊天内容的当前绑定快照，含 daemon epoch、connection incarnation、PID+birth、active mapping generation、turn/request identity、单调序列；断连/前台换代立即撤销；Deck restart 初始快照不回放旧完成提醒，gap 要求 resnapshot。
4. Deck 薄适配为现有 observation/episode/coverage，独立保留 authority/readiness。认证须覆盖 new/resume/fork、同 thread 多 client、reconnect、daemon/Deck restart 和 stale binding。没有这些证明，不将其标为 Trusted。

这些是缺失机制的验收要求，不声称当前上游已经实现，也不提出 headless orchestration。即使未来 attribution 修复，attention coverage 仍须单独认证。

## 13. Remaining unknowns / BLOCKED items

| 项目 | 状态 |
| --- | --- |
| 调查当日 stable / tag peel / source comparison | **PASS（调查）** |
| 源码 isolation / 实际两个 TUI 共用隔离 daemon | **PASS** |
| shared hook→triggering client→pane generation | **实际 FAIL；Trusted Signal BLOCKED** |
| embedded 普通回合 topology | **PASS（有界归属对照）** |
| /new、resume、fork、同 thread 多客户端 | **UNRUN**；没有修复迹象，不宣称全面生命周期通过 |
| TUI reconnect、隔离 daemon restart、observer/Deck restart stale binding | **UNRUN**；当前无可认证 binding，不能恢复旧关联 |
| ordinary request_user_input / Plan runtime | **UNRUN**；源码已有结构化请求，但 hooks coverage 缺失 |
| MCP / auth runtime | **UNRUN**；未配置外部服务，不操作真实登录 |
| continuation、后台 terminal + interrupt、全部 local UI boundary | **UNRUN**；源码已证明不能用 Stop/Interrupt/idle 泛化 |
| 安装版 Deck ingest / notification / scheduler 端到端候选矩阵 | **UNRUN**；独立 collector 结果不是该认证 |
| Rust/UI/coverage/release gates | **INAPPLICABLE**：仅新增调查文档和无内容证据，无产品代码变更；没有宣称测试套件已通过 |
| 内容隐私 / retained evidence allowlist / cleanup | **PASS（最终检查见下一节）** |

无法可信归属不是因为测试没跑完才保守：本次已有具体 B 反例。未运行扩展项不能推翻它，也不能被写成通过。

## 14. Cleanup verification

[cleanup.json](evidence/codex-signal-20261008/cleanup.json)、[停止结果](evidence/codex-signal-20261008/stop-result.json)、[清理前 generation inventory](evidence/codex-signal-20261008/pre-cleanup.json) 记录：

- 仅退出实验 A/B/E TUI；对明确隔离的 HOME/CODEX_HOME 执行 daemon stop，exit=0；停止本次独立 tmux server 与 collector。
- 清理前 inventory 的 PID+birth generations 最终均不存在，未按进程名批量 kill；初始失败 collector 也已退出。
- 删除临时 auth.json 副本、全部临时 HOME/CODEX_HOME、config、hook trust 状态、sessions/logs、work、tmp、managed daemon binary 副本、listener 与 tmux socket。
- 实际 daemon physical socket 及 alias 不存在；只删除本实验精确哈希的残留 lock；保留 UID 共用目录及其他内容。
- 调查源码 checkout、设置脚本和临时 investigation root 已删除并验证不存在；仅保留本报告及明确列出的无内容证据。
- 最终 worktree 相比初始仅新增本报告及 `docs/evidence/codex-signal-20261008/`；无产品源码、配置、既有文档变更、commit 或发布。没有停止/重启/替换用户 Codex、tmux 或其他服务。

**Can Deck now obtain trustworthy full Codex Signal from normal shared-daemon interactive Codex sessions without taking ownership of Codex execution? NO.** 0.161.0 仍缺少可信 interactive-client→current-pane-generation binding，并实测复现 B hook 的宿主环境继承和跨客户端 ancestry 断点；attention coverage 另有独立缺口。Deck owns attention around work. Codex owns the work.

<!-- All upstream references below are pinned to the examined full release commit. -->
[features]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L967
[attention-features]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/features/src/lib.rs#L1683
[matcher]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/matcher.rs#L1
[dispatcher]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/dispatcher.rs#L41
[thread-state]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/thread_state.rs#L318
[mcp-oauth]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/mcp.rs#L321
[goal-origin]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L844
[prediction]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread_prediction.rs#L1
[session-config]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/session.rs#L108
[nullable]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L40
[session-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L496
[prompt-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L564
[permission-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L298
[stop-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L585
[interrupt-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L624
[schema-enums]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/schema.rs#L838
[hook-events]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/protocol/src/protocol.rs#L1579
[session-identity]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/session.rs#L658
[session-root]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/session.rs#L909
[hook-build]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/hook_runtime.rs#L148
[thread-data]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread_data.rs#L204
[submission]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/mod.rs#L1110
[turn-start]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/request_processors/turn_processor.rs#L672
[turn-params]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/turn.rs#L174
[hooks-init]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/session.rs#L1617
[registry]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/registry.rs#L70
[command-runtime]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/command_runner.rs#L58
[discovery]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/discovery.rs#L125
[dispatch-exec]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/dispatcher.rs#L109
[command-spawn]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/engine/command_runner.rs#L386
[empty-env]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/utils/pty/src/child_command.rs#L94
[scrub-env]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/protocol/src/shell_environment.rs#L7
[posix-spawn]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/utils/pty/src/posix_child.rs#L200
[client-info]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v1.rs#L29
[connection-state]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/message_processor.rs#L176
[connection-id]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-transport/src/transport/mod.rs#L198
[subscription]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/thread_state.rs#L572
[running-resume]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/request_processors/thread_lifecycle.rs#L699
[loaded-list]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/request_processors/thread_processor.rs#L2777
[thread-read]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/request_processors/thread_processor.rs#L2824
[status-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L1665
[status-facts]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/thread_status.rs#L438
[daemon-recovery]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/daemon_thread_recovery.rs#L1
[background-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L1227
[environment-schema]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/environment.rs#L10
[thread-extra]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/thread_data.rs#L167
[input-inspect]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/hook_runtime.rs#L677
[turn-event]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/bespoke_event_handling.rs#L155
[approval-precedence]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/tools/approvals.rs#L505
[requests]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/common.rs#L1778
[request-input]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/bespoke_event_handling.rs#L825
[mcp-elicitation]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/bespoke_event_handling.rs#L872
[auth-elicitation]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/mcp_tool_call.rs#L758
[account-login]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-protocol/src/protocol/v2/account.rs#L814
[stop-continuation]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/turn.rs#L653
[turn-complete]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/bespoke_event_handling.rs#L1500
[interrupt-source]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/tasks/mod.rs#L805
[background-clean]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/handlers.rs#L58
[session-end]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/hooks/src/events/session_end.rs#L20
[session-shutdown]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/session/handlers.rs#L325
[home-resolver]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/utils/home-dir/src/lib.rs#L24
[daemon-paths]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-daemon/src/lib.rs#L324
[rendezvous]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server-transport/src/transport/unix_socket.rs#L48
[launch-policy]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/tui/src/daemon_startup.rs#L29

[goal-lock]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/app-server/src/thread_state.rs#L363
[plan-overlay]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/tui/src/chatwidget/turn_runtime.rs#L196
[session-end-root]: https://github.com/openai/codex/blob/979011409de0a60b52f179721948e65531d26144/codex-rs/core/src/hook_runtime.rs#L471
