# Deck v0.8.0：Pi 第一方集成详细设计

设计基线：2026-10-12；Deck `1bdcf57d7a9f7aebe989319af6d4dba0c7b57c4b`，初始工作区干净。Pi 官方稳定版 `1.1.0`，源码固定为 `abe508e1b89912adde45528136c3221eb69acdd7`。本轮只调查、原型及文档，不实现产品、不提交、不发布。

交付：[决策记录](decisions.md)、[完整 Implementation Prompt](implementation-prompt.md)、[独立验收清单](acceptance.md)、[调查证据](../../evidence/pi-investigation-20261012/README.md)。后续执行者必须一起阅读，不能只读本摘要。

## Executive Summary

**建议实现有价值的有限支持，不宣称完整 P1/P3 已可实现。** Pi 的前台进程内 Extension 可提供可归属的工作开始、交互结束及原生 Session 观察。首选方案是 **A：签名 Deck bundle 内的只读 Pi Extension 直接连接现有 Unix status socket**。保持静态 `AGENT_ADAPTERS`，不新增动态插件运行时、Agent 服务或任务管理。

两个独立的实证阻塞：

1. **P1 的可靠 `needs-input` 及解除等待 BLOCKED。** Pi `ui_prompt_start/end` 没有 request ID、源序号或公开的当前等待快照；两个通知独立异步分发，前面的用户 Extension 可延迟其中一个。真实 TUI 已复现 **end → 弹窗已关闭 → start**。Deck 本地序号、Socket FIFO、随机 UUID 都不能修复丢失的上游顺序。另有内置 UI 覆盖空白。因此 v0.8 的可执行子集为 **P1a：working/turn-done + 明示输入等待覆盖不可用**，Pi 自动 Prompt Delivery 保持 HOLD。原始三状态 P1 的验收仍为 BLOCKED，不能改标准取得 PASS。
2. **P3 精确自动 Re-entry BLOCKED。** npm 运行镜像是 Node；Node 的 Developer ID 不认证 Pi JS。下载并核验官方 SHA256 的 arm64 独立 Pi 是 `a.out`、ad-hoc、无 Team ID/封装资源签名。不能满足现有 Security.framework 供应方运行代码认证。**PI.reentry = None**，不能用文件摘要、路径、版本或用户勾选代替。

P2 可实现为**内核归属已验证的原生 Session 观察**，信任上限与 Signal 相同，不是供应方认证的重入回执。它与 P3 分开验收，不能让 `Observed` 隐式转换成 `VerifiedReceipt`。

| 范围 | 设计结论 | 当前证据边界 |
| --- | --- | --- |
| P1a run boundary Signal | 可实施，待生产验收 | 真实 npm/Bun CLI + 隔离 collector 原型通过；尚无 Deck ingest/UI 端到端 |
| P1 全部三种 Signal | BLOCKED | UI 通知乱序实测失败；不得发布“完整支持” |
| P2 lifecycle observation | 可实施，待生产验收 | new/resume/fork/reload/quit 实测，crash 无 shutdown 也已观察 |
| P3 automatic re-entry | BLOCKED | 两种官方分发均缺所需产品身份；另有精确 Session 前提 |

这里的“可实施”不是已实现 PASS。Implementation、Regression、Native E2E 在本设计轮均 **UNRUN**。

## Product Boundary Review

完整读取 `CLAUDE.md`、`AGENTS.md`；相关契约包括 `agent_status.rs`、`agent_origin.rs`、`agent_reentry/{mod,claude,lifecycle,launch}.rs`、`procinfo.rs`、`procinfo/code_identity.rs`、`tests/edr_quiet.rs`、`scheduler/select.rs`、Settings/Restart 前端头部，以及 first-party extensions、两轮 Codex certification、scheduler-context-safety、auto-respond、closeout reentry acceptance。

这项功能减少跨卡片巡查，观察 Agent 自己声明的边界，不解释任务意义。没有新的执行授权。新增仅有一个 Agent 的薄适配、观察来源和必要的健康/兼容性投影；Board、Attention、Authority、Readiness、Restart 的所有权不动。

禁止：读取任务正文、模型输出、工具参数、Session transcript；替 Pi 规划/继续/压缩 context；发 headless/RPC/SDK 任务；自动处理 UI；批准工具；建 Agent pool；管理 subagent/worktree/Git/PR；判断成功；自动移卡。原型的本地模型 fixture 仅测试真实 CLI，绝不进入生产。

保留宪法中所有现有能力和限制。特别是 `turn-done` 只是一次交互边界；P1a/P2 均无关闭、恢复、发送或解除授权的权限。Pi 的身份缺口不传播为 Claude/Codex 降级；Codex shared-daemon 也不因为新协议获得 Trusted。

## Current Architecture：实际代码而非候选假设

| 当前所有者 | 已核实的职责 / Pi 对接位置 |
| --- | --- |
| `extensions.rs` | 静态 `AGENT_ADAPTERS` 两项；重复 source 无赢家；注册不等于启用、信任、权限 |
| `agent_status::AgentAdapter` | source、SignalClass、hook spec/style/config path、lifecycle hooks、optional reentry；当前安装假设 JSON hooks，需要窄安装分支 |
| `agent_status::parse_event/ingest` | v1/v2 封闭状态；v2 必须是 SOURCE 自己的 UUID；不能塞观察者生成的 UUID |
| `agent_origin` | 同一进程快照的 peer ancestry、foreground generation、terminal continuity；Pi 不得修改这些条件 |
| `agent_status::Entry/Interactions/Evidence` | pane generation、交互、Episode、Codex trust、Claude first interaction；不能让新来源冒充 Claude proof |
| `handle_stream` | 一条消息后关闭连接；Lifecycle 单独分发给一个有界 receipt worker；慢代码签名调用不堵 Signal |
| `agent_reentry/lifecycle.rs` | 只有事务 lease/mailbox；非 armed 的生命周期被丢弃；无常驻 session registry；直接使用 Claude UUID 和 start/end 词汇 |
| `procinfo/code_identity` | 动态运行代码验证 + 文件参考 + generation，禁止版本/摘要名单替代 |
| `admission::interactive_agent` | 当前 claude/codex 闭集同时被 remote admission 和 scheduler 读取；新增 Pi 不能顺手扩大远端授权 |
| Settings / Restart UI | 固定 IPC reply keys；Claude lifecycle 开关；Restart 内部有 Claude 文案和全局 observation 标志假设，需要逐卡表达 |

当前 v2 自身还注明一个迟到 working(A) 覆盖 B 的未解决类别。Pi 不应扩大这个缺陷，更不能为它弱化 Claude/Codex 的现行行为。

## Pi API Investigation

以下源链接均固定到实查 release。官方入口 [pi.dev](https://pi.dev/) 指向 `earendil-works/pi` 和 `@earendil-works/pi-coding-agent`；不把旧 `@mariozechner` 或第三方 fork 文档当当前规范。

### 事件与身份

| 事件/API | 当前事实 | 集成解释 |
| --- | --- | --- |
| `agent_start` | 无 ID 字段；低层 agent run 可因 retry/continuation 再次 start | 开始/延续同一尚未 settled 的观察区间 |
| `before_agent_start` | 有 prompt/images/systemPromptOptions，无 interaction ID | 不订阅；避免把任务内容带进适配器 |
| `agent_end` | Extension 事件带 messages；session 公共事件另有 willRetry | 不订阅/不转发；不能判定 turn-done |
| `agent_before_settle` | 可请求一次继续并添加 context entries | 不订阅、不介入 |
| `agent_settled` | 1.1.0 带 aborted；retry、compaction recovery、queued continuation、before-settle 后才发 | turn-done；aborted 不等于失败/成功，不上传此任务语义 |
| `ui_prompt_start/end` | kind/title，无 request ID；只包 `ctx.ui.select/confirm/input/editor/custom`；外层聚合 depth | 当前不得生成可信 needs-input 或清除；丢弃 title |
| `session_start` | reason = startup/reload/new/resume/fork；新 runtime 的 ctx 指当前 Session | 生命周期事实，不是 working/readiness |
| `session_shutdown` | reason = quit/reload/new/resume/fork；指旧 runtime teardown | quit 也不是已退出证明；其余不是进程退出 |
| `ctx.sessionManager.getSessionId/getSessionFile` | 原生 ID、文件名（in-memory 可无文件）；不保证文件已落盘 | P2 只传 ID，不读取 transcript、不传路径 |
| `ctx.mode` | tui/rpc/print 等；hasUI 在 RPC 也可能为 true | 只接受 `mode === "tui"`，不以 hasUI 判 TUI |
| `ctx.isIdle()` | 反映 run/compaction，不覆盖 UI prompt | 不用作 ready/needs-input-clear |

依据：[事件类型](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/src/core/extensions/types.ts)、[UI wrapper 与 dispatch](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/src/core/extensions/runner.ts)、[run settlement](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/src/core/agent-session.ts)。

版本史由官方 changelog 复核：`agent_settled` 在 0.80.4；UI prompt 通知在 0.84.4；settled handler 中发起的新工作推迟到通知结束的修正在 0.87.0；1.1.0 新增 aborted。老 `session_switch/session_fork` 后事件已移除，应使用 `session_start.reason`。[变更记录](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/CHANGELOG.md)。这些是 API 兼容证据，不是产品认证名单。

真实 CLI 的重试/queued follow-up 是 `start,end,start,end,settled`；settled handler 再发起工作是 `start,end,settled,start,end,settled`，后者是新交互，不应通过静默延迟前一个结束来模拟 Agent orchestration。首次进入 Extension 但未开始 run 时不能凭 isIdle 合成 turn-done。

### 输入等待覆盖和乱序

runner 的 `withUIPrompt` 维护私有 depth，仅外层 start 和最终 end；每个通知 `queueMicrotask(() => void emit(event))`，emit 按 extension 顺序 await handlers。UI 本身不等待这些通知完成，因此通知可以互相越过。原型让较早 Extension 延迟 start 1500ms、弹窗 100ms 自动关闭，Deck 观察者收到 end 然后 start。独立架构审查得到同一结论。

嵌套/并行弹窗没有逐个 prompt 身份；内置 `/model` 实测不发这些事件。启动 trust、安装/更新、登录、外部 editor、任意 raw TUI、自定义终端输入也不能声称全部覆盖；即使部分 built-in MCP 通过 ctx.ui 发事件，不能泛化为完整覆盖。抛异常、取消、超时都走 wrapper finally 的 end，但分发仍可延迟。

**解除阻塞的要求**：上游提供在状态变更时编号的 prompt revision/instance、不可被其他扩展延迟打乱的观察接口，或有版本的权威当前等待快照，并明确覆盖范围。只有 request ID 仍不能证明延迟通知所描述的是当前状态。执行者不得 monkeypatch runner、拦截工具、重排用户 Extension、按 title/时间匹配来“修复”。

### Session 与 CLI

Session JSONL 当前 version 3；默认 ID 为 UUIDv7，但 `--session-id` 接受更宽的 `[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?`，实测非 UUID ID 成功。默认文件为编码 cwd 的 sessions 子目录中的 `<timestamp>_<id>.jsonl`；目录可由 `PI_CODING_AGENT_DIR`、`--session-dir` 或 extension session_directory 改写。

`--session <path|id>` 的细节至关重要：路径不存在时创建新 Session，甚至暂不写文件；ID 先本地精确查找，再前缀，再全局查找，全局可能弹出 fork 询问。ID 不存在 exit 1；`--session-id` 不存在会创建；`--continue` 选最近；`--resume/-r` 打开 selector。以上都不天然等于精确恢复。

CLI `--session` 恢复后的 start reason 实测仍是 **startup**；运行中 `ctx.switchSession` 才是 resume。/new、fork 改 ID；reload 保持 ID但换 runtime；tree navigation 可保留同一 ID。shutdown(new/resume/fork/reload) → 新 factory → start(reason)。旧 ctx/extension API 在替换后无效，异步回调不得继续读它。

来源：[CLI resolver](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/src/main.ts)、[session manager](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/src/core/session-manager.ts)、[配置与生命周期指南](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/packages/coding-agent/docs/extensions.md)。

### 分发与实际进程

npm CLI 是 Node 脚本，Pi 会设置 process.title，tmux 仍可能报告 `node`。名称差异不影响基于内核的 Signal 归属，也不能授予产品身份。两种 runtime 均实测直接连接 UDS；49 个内核 peer 采样验证了对应 shell parent、前台 PGID、tty（详见证据的计数结果），但没有运行 Deck 生产 ingest，不能称为生产认证。

官方 Bun compiled macOS arm64 归档 SHA256 与 release 的 SHA256SUMS 一致；codesign 显示 ad-hoc，无供应方 Team ID。官方构建脚本是 Bun compile 后归档，未见 Developer ID 签名步骤。摘要确认下载一致性，不证明供应方运行代码身份。[release](https://github.com/earendil-works/pi/releases/tag/v1.1.0)、[构建脚本](https://github.com/earendil-works/pi/blob/abe508e1b89912adde45528136c3221eb69acdd7/scripts/build-binaries.sh)。

## Requirements / Non-goals

R1：Pi 注册为 source `pi`；只做静态组合。R2：P1a 两状态准确归属，未知不伪造状态。R3：运行/transport 顺序独立于原生 Session ID。R4：跨 pane、旧 foreground、旧 instance、旧 run 无法覆盖新事实。R5：P2 纯观察、volatile、独立 opt-in。R6：Pi 自动发送 fail-closed，Signal 无授权。R7：用户配置字节保留、安装位置和 EDR 原则不变。R8：故障可见但不每次弹通知。R9：Claude/Codex 生产行为及测试不回归。R10：P3 不满足产品身份时没有 launch capability。

不做 Pi tool/UI/permission policy；不新增 API keys、网络服务、session 内容索引、完整历史、Task 状态、自动升级 Pi、全局 npm 安装或版本探测子进程；不试图认证每个第三方 Extension 的正确性。

## D1 / D3：Adapter、安装与传输

### 模块责任

建议新增 `app/src-tauri/src/agent_pi.rs`（Pi 描述、闭合协议、纯映射、配置入口）；若大小需要，仅拆 `agent_pi/config.rs` 和 `agent_pi/transport.rs`，不是公共插件框架。bundle 资源在 `app/src-tauri/resources/pi/`。共享 ingress/store/episode/attention 仍在 `agent_status`，生命周期观察归 `agent_reentry::lifecycle`。

`AGENT_ADAPTERS` 加 `agent_pi::PI`。`AgentAdapter` 的安装部分改为一个闭合 enum：`Hooks { specs, style, config_path, lifecycle_hooks, ... } | PiExtension`；现有两个 hook descriptor 原样搬入 Hooks，不重写 merge engine。新增 `SignalClass::Pi`，但普通 v1/v2 parser 对 Pi 拒收 `pi-observer-required`，不可用 helper/旧协议伪装 Pi frame。Pi descriptor 的 reentry 明确 None。

不新增前端 registry；IPC 在现有 fixed keys 上加 `pi`、`pi_lifecycle` 和 `pi_installation` 闭合状态（absent/current/needs-reload/conflict/unreadable/unsupported-location）。旧 keys、旧 Claude lifecycle command 均保留兼容；另加带 agent 的 lifecycle setter 或 Pi 专用窄 command，不能破坏现有调用者。

### 三方案比较与唯一选择

| 方面 | A 进程内直连（选用） | B status-helper | C 外部 wrapper/daemon/RPC |
| --- | --- | --- | --- |
| 开销/EDR | 无事件级 fork；一个现有私有 UDS | 每事件子进程；旧 helper 无 Pi 原生 ID，不支持清除/存活 | 常驻/更多 spawn；重构执行责任 |
| 归属 | kernel peer 就是 TUI；最短 ancestry | 父子链也可证明，但不能证明 JS 产品 | 多进程/共享服务重现 attribution 风险 |
| 隐私 | 构造允许字段，不读正文 | 必须拓宽 helper 契约才可满足 | RPC 通常暴露任务内容并有执行入口 |
| 生命周期 | stream EOF/teardown 与 runtime 对齐 | 单次调用不能独立证明 Extension 仍在 | 新管理责任及配置 |
| 复用 | 复用 socket、origin、store、episode；新增一种有理由的 framed ingress | 表面复用但 native-v2 语义不成立 | 无必要 |
| 故障/升级 | 有界队列、断线撤销、bundle 资源随更新 | 静默 exit0 不能报告覆盖缺失 | 更多长期状态 |

只实现 A。协议变化是源无 ID、session 重载、连接失效三项实际需求，不是第三种 Agent 的通用插件化。A 也不解决 UI 上游乱序或供应方身份；任何其他传输也不能解决。

### 配置的单一事实源

不在用户 home 写 JS/脚本/可执行文件；只对 Pi agent-directory `settings.json` 的 `extensions` 数组添加 bundle 内绝对入口路径。遵守 stable_installed_bundle；调试只能写通过现有隔离验证的临时 agent dir。默认配置目录用 Pi 公开规则解析一次，不遍历用户磁盘；若 Deck 环境没有 shell 私设的 `PI_CODING_AGENT_DIR`，不猜测、不读取 shell startup，显示“此 Pi 配置未接入”，不把全局安装成功声称为每个实例已加载。

两个开关使用同一 config 单一事实源，避免第二份布尔值：提供三个极薄入口 `signal.ts`、`lifecycle.ts`、`both.ts`，共同调用一个 observer factory，区别仅常量 `{signal,lifecycle}`。数组恰好引用其中一个；双关闭无入口。不是三条传输实现。切换 capability 以一次原子替换完成。未知用户项全保留；已有多个 Deck variants 为 conflict，enable/migrate 只合并 Deck 自有项，不能加载多个 observer 获得 last-wins。

使用现有 lossless JSON 编辑机制的 token/span 能力进行窄数组编辑；不能先 JSON.stringify 整个 settings。验证写后 parse 的值恰好等于预期；重复 JSON key、malformed、非数组、非字符串 entry、无法无损定位时原文件不动、报闭合错误。保留 symlink/mode；atomic_write_through_link；0600 新文件、0700 新目录；并发外部改写应比较原始 file identity/hash，冲突重读重算一次，仍冲突拒绝，不能覆盖 Pi 同时写配置。对可检测竞态须测试；跨进程缺共用锁的最后 rename 窗口作为已知残余明确报告。

所有权只识别受支持 Deck bundle 路径下这三个固定资源，不按文件名或模糊 substring 删除别人的 Extension。负向 `-<path>` 和用户显式禁用须保持关闭，不在 boot migration 重新启用。迁移只移动已正向启用的自有引用到当前合法 bundle，保持 signal/lifecycle 位；不注册从未启用的人。卸载只删除 Deck 引用，不删整个数组/用户目录，不卸载 Pi。

设置更新后后端立即收紧准入、撤销关闭的观察；旧 Pi 已加载的 code 不能被 Deck 远程卸载。变更在 Pi 下次启动或其自己的 reload 生效；UI 区分“已配置”和“此会话已连接”。Deck 不自动发送 `/reload`。

## D2：Signal 协议与状态机

### 为什么不能直接使用 v2

Pi 没有覆盖 retry/settlement 的原生 interaction UUID。Session ID 是会话身份而非 run identity；消息 entry ID 不存在于每个必要事件，且读取 transcript 属于错误方向。禁止把随机数伪装为 v2 native ID。

新增闭合协议 family `pi-observer` v1，仅同一个 `status.sock`；是 Pi 来源特有 framing，不改变 Signal 的三个状态词。与 `agent-lifecycle` 回执 family 保持隔离。

| Frame | 允许字段（不含公共 protocol/v/source） | 语义 |
| --- | --- | --- |
| hello | `source:"pi", socket, server_pid, pane, observerVersion:1, piVersion, mode:"tui", capabilities:{signal:boolean,lifecycle:boolean}` | 声明，不是证明；后端验证后赋 stream lease |
| signal | `seq, run, state:"working"|"turn-done"` | 观察者 scoped run；不含原生 ID |
| reset | `seq, reason:"new"|"resume"|"fork"|"reload"|"quit"|"fault"` | 撤销当前观察，不是第四种 Agent 状态 |
| lifecycle（P2 才启用） | `seq, event:"start"|"shutdown"|"snapshot", nativeSessionId`；start/shutdown另外要求`reason`，snapshot禁止reason | 仅观察；start/shutdown reason分别按上游闭合枚举验证；snapshot只是重连后的当前API读取，不伪称新建/恢复 |
| ping | `seq` | transport 存活，不是 Agent idle/readiness |
| server reply | `accepted` 或闭合 `refused` code；帧 ACK 含已处理 seq | 没有 prompt、命令、工具、session 操作能力 |

hello 字段均有 bounds：line≤1024 bytes；piVersion≤32 的正式三段版本；pane/socket/server_pid 沿用现有 validator；任何重复/未知 key、NaN、负数、分数、溢出和重复 seq 拒收。后续每行≤1024，整数≤JS safe integer；seq 从1严格连续，run 从1单调。未知 protocol/version fail-closed。API compatibility 初始只开放 1.1.x，认证样本仅1.1.0；未来区间扩展需源码和运行验证，不拿版本替代 kernel/产品身份。compatibility gate 使用公开 VERSION export，不 spawn `pi --version`。

Extension 在 session_start 后、mode tui 且 DECK_STATUS_SOCK/TMUX/TMUX_PANE 都存在时才连接；不向 `~/.deck/status.sock` fallback，避免隔离实例误投生产。factory 仅注册回调，无 socket/timer；shutdown先读取允许的最后ID（若P2开），立即标记 disposed、停止timer和后续回调，再排入最后lifecycle/reset并最多等待100ms drain后关闭socket，不能无限阻塞Pi。超时没有end回执，后端按EOF撤销，不补造正常退出。一次 factory 一条连接；初次 session_start 没有工作状态。

DECK_STATUS_SOCK只作为Unix path参数使用（绝不解释host/port/URL），要求绝对路径、无NUL、平台长度以内；连接前核对socket由当前UID拥有且0600、所在目录private。它仍是路由声明，不是Deck供应方认证，不承载任何Prompt内容；同用户进程内Extension已有的任意代码能力不被本协议伪称sandbox。

首条 hello 由现有 accept loop 读出后交给**一个有界 Pi transport 服务**，不让长连接阻塞 Claude/Codex。复用已有 Tokio，不新增依赖/监听器：一个 owner 管理最多32条 async streams；总待处理队列≤128、每连接≤32帧，一条 admission worker 顺序处理，拒绝过载并撤销对应 stream。不能每事件开 thread、无限 spawn_blocking 或让一个坏客户端占住 accept loop。读首行和 idle/read/write 均有界；帧 ACK 要在验证和 commit 后，不得照搬旧 close-before-ingest 当成功。

客户端同步回调只构造 content-free frame/入队，不 await Socket 或抛错给 Pi。一次只发一个待 ACK 帧；2秒未应答/溢出关闭并忘记队列，不重发历史结束。心跳5秒、后端15秒无有效帧撤销 coverage（超时只证明观察不可用，绝不合成 done/idle）。断线重连按1/2/5/10秒上限尝试、timer unref；无无限内存。shutdown 后无重连。Deck 重启时新连接不回放旧 turn-done；当前仍在 run 可重发 working 为新 lease/run1，空闲则只 hello，等待下一个真实边界。

### 身份与并发准入

每个 stream 在连接时取得 kernel peer PID 和 birth；每次 frame commit 都重新验证：server/socket、pane/session/pane_pid、当前 foreground(pid,birth)、peer live birth、pane shell ancestry、terminal_continuous。**Pi 直连额外要求 peer == foreground leader**；不支持 detached child observer 或 wrapper leader，不为兼容降低祖先/tty约束。本轮两种 runtime 符合，但 wrapped launch 未认证，要诚实 unavailable。

backend 为通过验证的 stream 赋不可复用 lease。观察身份为 `(PaneKey, pane/session generation, foreground generation, lease, run)`，明确是**观察通道内的交互区间**而非 Pi 原生 turn ID。共享 store 的 interaction key 使用闭合 enum：现有 NativeUuid 路径保持原逻辑，PiRun 路径只接受经 Pi ingress 验证的 lease/run。不能把新通道 parse 成旧 v2。

一个 foreground generation 同时最多一个活跃 observer；第二连接在旧连接仍有效时不替换（保持旧，给新 duplicate-observer）；旧正常 shutdown 或 EOF 后新 lease 才能进入。所有 commit 带 lease，旧 worker/ACK/延迟 frame 在替换后无效。reset 撤销该 lease 的 Signal 和 Episode，不影响另一个 pane/Agent。没有失效重放、跨连接 run 合并、按 native ID 去重 pane。

核验在数据写入时仍需当前 lease；慢 listing 后重新读 generation，不能把旧 snapshot 当“现在”。失败事件不为被冒名的其他 pane写 unavailable。只有已绑定 stream 的失效，才能撤销自己的状态；unknown sender 仅闭合诊断。

### 事件映射及影响

| 场景 | 上游事件 / 状态迁移 | 顺序与身份 | Attention / scheduler / failure |
| --- | --- | --- | --- |
| 开始工作 | agent_start：无 active run 则 run+1，working；有 active run 则保持 | lease+run；不看 prompt/session ID | 共享 working 投影；Pi 自动发送仍 HOLD |
| 请求交互 | ui_prompt_start 当前 BLOCKED | 不制造 pairing/native ID；不传 title | 不伪造 needs-input；卡片静态说明“无法可靠观察输入等待” |
| 交互完成 | ui_prompt_end 当前 BLOCKED | 不清除其他 run/状态 | 不因它解除 scheduler；未来需上游修复再设计 |
| 正常结束/中止/错误结束 | agent_settled 且确有 active run → turn-done，关闭 run | 仅当前 run，重复 settled 不生新 episode；不能无 start 合成结束 | 共享 episode/attention；不判 task success/ready |
| 自动 retry/compaction/follow-up | 中间 agent_end 忽略；多个 agent_start 同 active run | 不提前结束、不计时猜测 | 保持 working；不释放行 |
| settled callback 发新工作 | 旧 settled → 新 agent_start 新 run | 原生 await/defer 顺序；真实两边界 | 合法新 episode，不能合并成任务语义 |
| switch/new/fork | shutdown(reason) → reset/dispose；新 start/lease | 旧 native ID 不能移给新连接 | 清除旧 Signal；新 session不制造 done |
| reload | 同上，可能同 native ID，新 lease | 不复用旧 run；重入 API 对象禁用 | 短暂 unavailable、加载后恢复有限 coverage |
| 正常退出 | shutdown(quit) reset；随后 OS generation 消失 | quit不是 process gone；reconcile最终清理 | 不移卡，不推断成功；普通 shell_exit 规则原样 |
| 异常退出/缺失/禁用 | EOF/health timeout 或从未 hello | 未知不关联其他 pane；当前generation清理 | unavailable；无伪造 needs-input/done；HOLD |
| Socket失联/过载/Extension内部错误 | 丢弃 pending，不发送最终状态；reconnect新 lease | 不回放旧 settled；新事件重新开始 | unavailable，低干扰；无自动 prompt重试 |

P1a 从生产 Extension 根本不订阅 ui_prompt/agent_end/before_agent_start/tool_call/tool_result，因此没有携带这些 payload 的偶然通道。UI 反例保留在 test fixture，未来改进 API 后复认证。

### Scheduler / Readiness 明确边界

Pi 当前输入等待覆盖不足，因此对已识别 Pi 的所有**自动**行增加独立 hold `pi-input-unverified`，在所有 readiness override 之前执行；working/turn-done/hello/lifecycle/ping 均不能解除。Send now 仍为用户操作，遵守全部现有 target/foreground/paste guard，不是普遍 bypass。

识别只用于加 HOLD：row.expected_process 为 pi、pane foreground basename为 pi，或当前 generation有已绑定 Pi observer evidence。node 名称不认证 Pi，不能把所有 node 纳入或接受为 pi 的交付目标。未配置、没有 observer 的任意 `node script.js` 无法知道是不是 Pi；这种启动形式不在第一方 Pi delivery支持范围，不得宣传“所有 Node Pi 都安全自动化”。

不把 pi 加进 `channel_agent_command` 远端允许表。现有 `admission::interactive_agent` 同时用于远端/启动门禁，不能直接扩表；本轮在 scheduler 的已识别 Pi 分支加入 hold与 fresh-start只启动不发送路径，保留 remote闭集。Pi不获得现有first-send风险覆盖的继承权，不增StepAuthority/TriggerClass。所有pre-fire路径再次检查hold，不能只在tick检查。

## D4：Session Lifecycle 与现有 Claude 耦合

P2 使用同一已归属连接、同一 seq，调用 `lifecycle::observe_pi`，不另建 listener/session框架。只存当前 generation/lease的 `NativeSessionKey { source:"pi", id: PiNativeId }`、最后 start/shutdown闭合reason和availability；进程内内存，按活跃pane限额，reset/EOF/foreground变化/disable清理。无永久session registry、history、索引、恢复journal；原生ID不进入日志、UI、disk、notification。

PiNativeId按上游字符集验证，最大256字节（产品边界，超长应报 unsupported-native-id而非截断/哈希假ID）；Claude保持它自己的lowercase UUID。不同agent相同字符串永远不同key；同一Pi session两个pane各自有独立process/lease观察。P2不需要session路径：默认路径不可靠、读文件也不产生身份；只在真正设计P3时审批其短期file reference。重连仅在旧ctx仍有效时读取getSessionId发送snapshot（不可用就保持unknown），不重放session_start，也不从磁盘重建关联。

`session_start` 观察更新当前原生ID；`shutdown`必须属于同一lease和ID，原因不匹配/同lease换ID而无teardown则撤销，不能猜哪个更新。`new/resume/fork`换实例；`reload`换实例可同ID；`quit`仅“runtime关闭”；crash只有进程消失不能补“正常退出”。`session_info_changed` 的name不是identity，不订阅。

**解除耦合的具体范围**：当前 `lifecycle::parse(known_source: bool)` 只检查“有reentry”却硬编码Claude形状，未来第三项危险。将这个bool改成adapter的**显式receipt contract**查找；把Claude字段解码、StartSource/EndReason、`final_session/reentry`的Claude语义放回`claude.rs`。`lifecycle.rs`保留wire framing、origin检查、lease、queue、deadline、mailbox和所有相同fences。给 `ReentryCapability`增加一个静态 `receipts: &ReceiptContract`，含纯decode/final_session/entered reducers；只Claude提供。shared Fact的native语义用闭合tag包裹，不让Pi ID过Claude validator。

Pi P2经 `observe_pi` 到同模块的独立**观察记录类型**，不提供ReceiptContract，不允许转换Fact进入reentry mailbox；不是第二套restart框架。此次不实现Pi receipt/restart reducers，也不为“未来通用化”提取provider trait。可以保留文件分拆，但测试必须证明non-armed观察和armed回执不串路，慢Claude签名验证不阻塞Pi或旧Signal。

P2不依赖P1a启用：lifecycle-only entry连接，signal帧不允许；signal-only不得取得/发送native ID。共享session_start/shutdown用于transport边界不代表生命周期opt-in。

## D5：Re-entry 条件设计与阻塞传播

当前 UI/后端：Pi显示 `Unavailable — client identity cannot be verified`；不勾选、不发token、不发exit key/launch，不加入自动恢复候选。若只有名称提示、无法确认Pi则用通用unsupported，不把名称猜测升级身份。混合review的Claude继续其原事务，Pi仅保持shell-only restart语义及现有影响确认。

未来要开放P3，必须单独关闭下表全部前提，不能在本Implementation Prompt中实现占位launch：

| 步骤 | 必须具备的证据/行为 | 当前缺口 |
| --- | --- | --- |
| 资格 | 用户逐卡选择；native lifecycle opt-in；唯一pane；直接TUI；可认证供应方running code；支持launch grammar | npm只证明Node；Bun ad-hoc：BLOCKED |
| 旧身份 | Security.framework动态+静态binding、pid/birth、文件state/hash；升级/替换/exec前后再次核对 | 不允许新增Node通用签名requirement冒充Pi |
| 原生目标 | 旧runtime报告最终ID和精确文件引用；是持久化session；文件header的原生ID严格一致、目标冻结 | P2观察不能作为该回执；未写session不可恢复 |
| 退出 | shutdown(quit)且旧客户端generation确实消失；new/resume/fork/reload均不算最终退出 | Pi.quit不是用户restart专属理由，需事务lease关联；不能只等EOF |
| 精确请求 | 优先绝对现存Session文件；身份/权限/目录冻结；构造 `<frozen Pi image> --session <frozen path>`，无prompt/no-session/fork/continue | 当前launch词法只允许alnum/hyphen，path需独立安全审查；Node不能重放任意script argv |
| 一次发送 | 复用现有freeze/Board fence/consume slot/clean shell/paste+Enter；不确定不重试 | 不能为Pi复制第二个coordinator |
| 新身份和回执 | 新foreground再次同产品running code；argv精确；startup receipt+确切ID/文件证明其进入目标 | CLI resume事件是startup，不能套Claude resume；缺少file存在时失效而非创建的CLI保证 |
| 超时/部分成功 | 未launch=Skipped/Refused；已可能发送=Unconfirmed，保留client，不补Enter、不重发、不自动kill | 与Claude同等级，不得叫任务恢复/成功 |

即使将来有签名独立Pi，现存文件在precheck到Pi读取之间可被删改，当前`--session path`会新建。这至少导致Unconfirmed且可能原生副作用，不能在设计中隐瞒为“missing就不启动”。需上游fail-if-missing/exact-id验证原语或单独审查该窗口；P3本轮不接受这个新残余。npm供应链provenance/integrity是安装证据，不是运行中的JS绑定；自签Pi或Deck自建分发会改变供应方信任，均属单独决策。

现有宪法`--resume`表述和当前launch grammar针对已实现Claude。真正引入Pi `--session`前还应审查宪法表述是否需产品决策；本轮不修改宪法，不借“adapter通用化”绕过。

## D6：UX

Settings沿用Agent Status分组，新增“Pi工作状态”和“Pi会话观察”，均默认off。Signal说明固定为“显示工作开始和交互结束。当前无法可靠观察输入等待；自动发送暂停。”Lifecycle说明“观察当前原生会话；此安装暂不支持自动重入”。开启使用现有设置确认模式；关闭不额外确认。不显示ID/hash/PID/协议名，不要求用户理解transport。

安装状态和live状态分开：未安装、已配置/等待Pi加载、已连接（有限覆盖）、不兼容、配置不可读、观察不可用。不能把设置中的路径存在当extension已加载；只能由通过kernel证明的hello显示对应当前pane已连接。仅有command hint而没hello显示未连接，不猜是缺失、禁用还是加载失败；不能精确诊断的统一“Pi状态不可用”。

卡片继续同一working/turn-done状态与Episode；输入等待覆盖提示为低干扰详情/tooltip，不算Needs attention数量，不触发away notification/Dock badge。真实turn-done仍正常进入现有attention，不因为有限覆盖一律静默。断线不得留一个看似仍实时的旧working/done；撤销episode并展示不可用。不得用灰色“无状态”暗示任务已安全结束。

Restart Review每卡给agent label和closed reason；Pi不可选择，原因独立于Claude的observation开关；勾选Claude或Policy B不会选择Pi，不改Pi权限。P2开关打开后仍不能变成reentry可用。不新增重试、复制自动执行命令、重新批准工具之类入口。

中英文同步，最小1280×800、font scale、长名称、Pi/Claude/Codex/普通shell混合、部分不可用都进入WK测试。保持frontend backend IPC unknown code fallback，任何free-form runtime错误不得进UI日志。

## D7：Security / Privacy / Failure Handling

| 威胁/故障 | 必须控制 |
| --- | --- |
| 跨pane伪造TMUX_PANE/同cwd/同session ID | kernel peer+birth、真实parent、当前FG、tty全部验证；标记不够 |
| subagent/后台工具继承环境 | Pi直连只准peer等于FG leader；无继承环境授权 |
| 旧generation、旧instance、重复seq/run、延迟ACK | lease+generation commit fence；闭合序号校验；旧数据不回放 |
| 用户同进程Extension恶意发送 | 同pane presentation残余明确接受；不给自动输入/重入/工具权限；不是extension sandbox |
| UDS本地攻击者/大量连接/大行 | 0700/0600、peer核验、行长/速率/queue/连接上限；bounded日志；满队列诚实unavailable |
| UI事件缺口 | 不使用错误状态；固定coverage提示+自动发送hold，不能以缺失当safe |
| 安装更新/禁用但旧进程仍运行 | 后端当前配置收紧立即生效；旧capability不扩大权限；bundle替换不是新Pi产品身份 |
| Extension抛错/卸载/reload | wrapper try/catch仅闭合fault、dispose、释放；不吞掉后端availability；没加载不能假诊断具体原因 |
| 日志隐私 | 仅闭合reason、计数；无prompt/title/messages/tool/nativeID/path/version原文/证书/原始JSON |
| Pi Session文件包含敏感内容 | P1不读，P2只API读ID；不扫描目录、transcript，不向前端暴露ID |
| Deck停止/Pi继续 | observer断线无影响Agent；重连不恢复历史attention/authority |
| 健康计时 | 仅撤销观察；不能合成Agent语义、放行输入或推断正常退出 |

Configuration问题与runtime问题分开闭合错误，持久化失败不保存默认覆盖旧文件。资源包升级须在build.rs/tauri bundle资源中固定并验证，不能写到home再import。增加TS生产资产扫描到EDR/privacy gate，禁止child_process/exec/spawn、网络HTTP、tool/session mutation API和正文访问；不能只扫描Rust而漏掉实际运行的Extension。

## Implementation Plan / Compatibility

执行顺序：先P1a installer+direct transport+shared observation+Pi input hold+UX，完整验收有限功能；然后P2 volatile lifecycle projection和Claude receipt解耦；最后仅实现P3不可用的正确展示/负向测试。不要先实现P3再以弱身份补洞。

P1完整BLOCKED不阻止P1a、P2有用交付；P3BLOCKED不否定任何已验收观察。P2不得使Signal开关默认开或发送nativeID；P1a不要求生命周期持久化。三阶段是验收范围，同一transport/config/resources，不保留三套框架。

不增加Deck settings持久化布尔或schema，不改现有队列authority/trigger closed vocabulary；新增hold属于已有closed result/stage层，逐个审查是否有持久化枚举，若需要schema门必须明确理由不能静默旧reader误解。安装外部Pi config只有路径变化，可卸载回到原文本；旧Deck不懂Pi family直接拒绝，不能fallback v1。回滚旧Deck后不承诺Pi自动输入保护仍存在，release说明必须标注；不得迁移新权限到旧版本。

## Test Plan / Acceptance Criteria

详细逐项可执行矩阵和证据要求见[验收清单](acceptance.md)，执行说明见[Implementation Prompt](implementation-prompt.md)。以下是不能用mock代替的边界：

- Unit：闭合parser、双family、seq/run/lease状态机、malformed配置字节保留、source-specific ID、Hold/episode/census。
- Integration：真实Unix socket的kernel peer、两个pane同项目、伪造pane、旧foreground/late worker、连接过载/断连、Claude receipt与Pi observation严格隔离、配置跨family独立。
- Native Runtime：真实Pi1.1.0 npm与独立binary、interactive TUI、本地deterministic provider，不需私人credentials；retry/follow-up/settled-reentry、UI乱序负例、new/resume/fork/reload/quit/crash、missing/no-extension、实际配置加载。
- WKWebView E2E：现有debug carrier、真实settings IPC写临时Pi config、生产backend admission到卡片Attention、Restart Review混合非候选和Claude行为；mock只算layout不算E2E。
- Safety/Regression：全workspace/前端gates，signal_census、edr_quiet、log_privacy、external_admission、procinfo identity、Claude reentry、Codex shared/embedded、experiment cleanup。改动后必须真跑，不能复用本设计原型充数。

PASS必须对应最终commit、命令exit code、期望与实测、build/source/fixture版本及可读证据。P1全量和P3本轮的阻塞只能被新的上游证据关闭，不能用通过negative test替代功能PASS。不能自动运行的签名release/macOS版本矩阵单独BLOCKED；已有可自动运行的WK/原生tests不能借此跳过。

## Known Limitations / Open Risks

已证实：UI源事件乱序、内置UI不全、产品身份缺失、路径不存在会创建、CLI恢复reason是startup、nativeID不限UUID、没有native run ID。无需再让执行者花一轮“发现”这些事实。

待生产验证：持久stream新worker的容量/公平性；live config禁用与stream竞态；升级后的resource路径；frontend coverage投影；Debug真实WK的UI操作；Pi故障下worker清理；Claude receipt提取不回归。本轮原型采用一次事件一个socket，仅证明direct peer拓扑，不证明拟议长连接的实现。

接受的有限残余：同pane代码可自报错误观察；供应方在同API形状下改变语义不能仅靠版本发现；一般socket/进程观察到显示有延迟；没有loaded observer的间接Node调用无法自动识别。均不授予任何新执行权。**不接受**用这个残余解释UI已经复现的确定乱序后仍发布needs-input，也不接受Node/Bun路径当Pi产品身份。

尚需独立验收而非额外用户操作；本设计不要求用户安装工具、登录模型、点击测试UI或改真实配置。执行者应自动修复隔离测试设施；确有系统能力缺失则准确报BLOCKED，不降低门禁。
