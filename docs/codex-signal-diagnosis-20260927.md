# Codex Signal 覆盖诊断（2026-09-27）

结论：可以改善并分阶段恢复 Codex 的状态覆盖，但最新版并没有自动解决共享 daemon 的归属问题。保持现有拒收边界是正确的；把拒收后的完全沉默当作产品终态则不够。应分别处理「用户知道状态不可用」「可验证的终端状态展示」「完整、可归属的交互事件」。

本文件记录最初的诊断与方案评估阶段；当时未改变应用代码、Codex 配置、运行中的会话或发布状态。用户随后批准执行，后续结果见[实现与验证记录](codex-signal-implementation-20260927.md)。工作区原有的 `CLAUDE.md` 修改保持不变。

## 证据范围

- 本机 `codex --version`：`0.157.1`。GitHub latest release、npm latest 和官方 changelog 均指向该版本；发布日期为 2026-09-26。
- 本机 `codex features list`：`daemon_auto_start` 和 `hooks` 均为 stable / true。
- 检查了上游 `rust-v0.157.1` 源码快照（目录 commit 前缀 `ac0e23e`），不是根据版本说明猜测实现。
- 读取 Deck 的相关提交、nightly tags、Signal/helper/投影代码与验收要求。
- 本机用户 hooks 文件中仍有 Deck 的四个异步 hook，均指向安装包 helper。这个检查只证明配置存在，不证明已经通过 Codex 的 hook trust 或会实际执行。
- 用安装包的 bundled tmux 只读查询当前 pane 标题，在内存中分类：发现两个 foreground 为 `codex` 的 pane，其中一个包含 Codex 活动 spinner；未发现明确运行状态词或完整 UUID。没有输出或保存标题正文、会话文本。
- Deck 日志的有界、无内容计数中存在 `terminal-discontinuity` 拒收记录，也有历史 accepted Codex 事件。混合历史不能证明当前所有 pane 的模式或健康状态。
- 未运行新模型任务，也未完成新版双客户端功能验收、权限/提问/中断/重启矩阵。以下严格区分源码可行性和运行时认证。

## 过去的方案究竟解决了什么

| 版本节点 | 关键提交 | 行为与意义 |
| --- | --- | --- |
| 0.7.14 nightly | `2ee3397`，候选 `8132cf6` | Signal Integrity：回合结束不等于任务完成；撤销基于 Signal 的自动关闭/外部续发授权；加入 pane/前台 generation、交互 ID、统一 attention episode。 |
| 0.7.15 nightly | `a68f686`，候选 `a1bb9d0` | 拒绝共享 daemon 跨客户端误归属；新增 terminal continuity 和按 generation 的 Codex 信任降级；未证明可信时暂停自动发送。 |
| 0.7.16 nightly | `e24651f`，候选 `521bbd3` | 新 agent 首次交互之前不自动输入，防止把提示词送进启动/信任对话框。 |

`app/SMOKE.md` 已明确：embedded Codex 是功能验收，共享 daemon 的 `codex-daemon-refused` 是安全降级验收。后者通过意味着没有串卡/误发送，不意味着用户能知道工作进展。

现有 `agent_status.rs` 的内核 peer PID、pane 进程、前台 generation、terminal continuity 检查应保留。`turn-done` 仍只表示交互边界，不能表示任务成功、无后台工作或可以关闭卡片。

## 0.157.1 仍然存在的归属断点

上游 `hooks/src/registry.rs` 的 `Hooks::new` 从运行宿主的 `std::env::vars_os()` 建立环境快照；`hooks/src/engine/command_runner.rs` 执行 hook 时 `env_clear()` 后重放该快照。共享 app-server 中的宿主是 daemon，不是正在操作的每一个 TUI 客户端。

因此，hook 的 `TMUX_PANE` 仍可能是启动 daemon 的 pane。`session_id`、`turn_id` 能识别 Codex 会话/回合，却没有建立「这个会话现在属于 Deck 哪个 pane、哪个前台 generation」的可信关系。单纯让 helper 再读一个 ID，无法补上这个断点。

本次检查的初始化协议 `ClientInfo` 只有 name/title/version，没有 pane 或客户端 PID 绑定；hook 输入 schema 也没有可直接替换现有内核归属证明的终端绑定。未发现可直接接入、满足当前约束的 per-client hook binding。

这不是建议恢复原来的宽松判断：daemon 位于启动者的进程树中，单看祖先链依然可能把其他客户端的事件画到启动者卡片上。

## 可用路径及其边界

### 1. Embedded hooks：近期最明确的兼容路径

当前 CLI 提供 `codex --no-daemon`，帮助和源码均明确：即使共享服务已经存在，也不发现/连接/启动它，使用 embedded 模式。它保留交互式 TUI，能复用 Deck 已有的 hook 归属模型，不需要 Deck 接管模型调用或 Agent 工作。

与 `--disable daemon_auto_start` 不同，后者主要禁止自动启动，已有 daemon 的复用还需要另行考虑；不能为了验证而停止用户共享服务。现有 SMOKE 中先停 daemon 的旧操作应在下一次维护时改为隔离的 `--no-daemon` 测试。

建议提供清楚说明代价的可选兼容启动方式，先对 0.157.1 重跑功能矩阵。不要静默改写用户命令或全局配置，也不要把它变成使用 Deck 的强制前提。代价是退出共享后台服务模式，不再享受该模式的会话持续运行/跨客户端使用方式。

Hooks 本身还有覆盖边界：当前 Deck 的 `needs-input` 接的是 `PermissionRequest`，不能据此承诺覆盖所有普通提问、Plan 选择或 MCP elicitation；Stop 仍可能是尝试结束，Interrupt 仍不保证后台终端停止。恢复既有 hooks 不等于所有用户等待情形都已覆盖。

### 2. TUI 终端标题：值得验证的展示补充

这是绕过 daemon 环境继承问题的现成来源：状态由客户端 TUI 向自己的 stdout 发出 OSC 0，tmux 保存到相应 pane。上游已经具备：

| 来源 | 能表达什么 | 不能推导什么 |
| --- | --- | --- |
| `activity` | 工作动画；`[ ! ] Action Required` / `[ . ] Action Required` | 没有动画不证明回合结束；动画还受配置影响。 |
| `run-state` | `Starting`、`Ready`、`Working`、`Thinking`、`Waiting` | `Ready` 不等于任务完成；`Waiting` 可能是等待后台终端，不等于等用户。 |
| `thread-id` | 当前会话的显示标识 | 实现把标题段截到 32 个 grapheme，36 字符 UUID 会成为前缀加 `...`，不是完整身份。 |

`Action Required` 的来源覆盖 approval overlay、request_user_input、MCP elicitation、异步问题等，比当前单独的 PermissionRequest hook 更贴近用户注意力需求。

但默认标题仅有 `activity/thread-name/project-name`，并没有 `run-state`。不能解析任意自由标题中的词并当作封闭状态；项目/会话名本身可能含有相同文字。若做原型，应使用明确选择的、只包含受控状态项的格式，只提取封闭词，不把标题正文、项目名或问题文本写入诊断、通知或持久化。

需要先解决的可靠性问题：

- pane title 是缓存。进程退出、前台换代、Deck 重启后仍可能留下旧值；必须证明新鲜度，不能把首次读到的 Ready 当成刚结束。
- 轮询会漏掉两次 poll 之间的完整短回合，且标题没有 turn_id/顺序号，无法直接复用完整的 SI-04 交互语义。
- 任何能向该终端输出的程序都可能修改标题。pane 归属强于 daemon 继承环境，但标题本身不是 Codex 身份证明。
- 应覆盖未显示/未 attach 的卡片，不能只在当前 xterm 的前端回调里观察，否则离开卡片又失明。
- `/new`、resume、fork、同一 thread 被其他客户端打开、标题关闭或自定义、动画关闭、多 pane 切换都需要验证。

结论：适合先做版本限定的状态展示原型。只有新鲜度、语义和跨 pane 验证通过的观察才考虑进入 attention；不能直接升级 `CodexSignalTrust::Trusted`，更不能解除队列发送/首次交互门禁。

测试陷阱：0.157.1 的 daemon exclusion 会把许多 `-c` 覆盖（包括标题配置）排除到 embedded 路径。用 `codex -c tui.terminal_title=...` 得到正确结果，不证明共享 daemon 下正确。共享模式原型必须采用隔离配置并实测进程拓扑，不能只检查 feature flag。

### 3. App-server 状态：语义更好，但身份还需补齐

官方协议已有 `thread/status/changed`，区分 idle、active、systemError、notLoaded，以及 `waitingOnApproval` / `waitingOnUserInput`；`thread/read` 可以不包含 turns。这是有价值的未来只读观察来源。

然而知道某个 thread 的状态，不等于知道它对应哪张 Deck 卡。按 cwd、最近活跃、唯一看似匹配的线程、截断标题 UUID 或 rollout 文件猜测，都会在同目录多卡和 resume/fork 场景失效。

不应为了订阅而默认调用 `thread/resume`：协议说明它可以加载/重入线程，并带运行配置变更；这不是纯状态读取。普通 app-server 客户端能力也不自动等于权限被限制的 observer。

长期建议优先争取或验证上游 TUI 的结构化、无内容、完整身份状态通道：至少有客户端生命周期绑定、完整 thread/turn ID、顺序/重连边界、working/需要用户/交互结束，以及恢复当前快照的方式。Deck 只观察和投影，不启动 turn、不回答批准、不接管会话。

### 4. 不宜作为主方案的来源

- OSC 9 / BEL 通知：TUI 确实有结束/批准/问题通知，但受通知配置、焦点和 backend 影响；OSC 9 发的是可能包含回复/命令的展示文本，没有稳定的封闭事件/交互 ID。不能据此保证完整覆盖。
- legacy `notify`：仍无法解决 daemon 归属，而且占用用户唯一 notify 配置，之前放弃它的理由仍成立。
- 扫描聊天文本/rollout 或依据输出静默：不满足语义、隐私和稳定性要求。官方也声明 transcript 格式不是稳定 hook 接口。

## 另一个独立缺口：安装成功不等于信号生效

当前 Codex 对非 managed hooks 要求用户在 `/hooks` 中信任准确的定义哈希；定义变化后可能重新变成待审查。Deck 的开关来自 hook 配置是否已安装，不能证明 hook 已可信或执行成功。不要用 bypass-hook-trust 绕过用户审查作为产品修复。

同时，`CodexSignalTrust` 当前主要服务 scheduler，没有作为 coverage 投影到卡片；未观测到 hook 时，卡片回落到 `effectiveCardStatus` 的输出活跃/15 秒静默启发式。用户因此既没有可信结束提示，也不知道为什么没有。

应展示低干扰的「Agent 状态未连接/不可用」，附简短诊断入口：检查 `/hooks`、模式兼容性和最近是否接收到有效事件。Unknown 只能说未建立信号，不能武断断言是 daemon；拒收原因也不能套到没有证明归属的其他 pane。该提示本身不增加 Needs Attention、Dock badge 或系统通知。

## 建议的实施顺序与验收

1. **先补覆盖可见性与诊断。** 保留 working/needs-input/turn-done 的含义；将 coverage 与工作状态分开表达。验收：信号缺失不会静默伪装成可靠状态，也不会每张卡都触发通知。
2. **认证可选 embedded 路径。** 0.157.1 的正常结束、批准、问题、中断、后台任务、连续回合、重启、hook 未信任分别验收；标清 hooks 仍覆盖不到的等待类型。
3. **做共享模式终端状态原型。** 两个同目录客户端交错运行，证明每张卡只得到本 pane 的观测；覆盖离屏卡片、短回合、重启/换代/缓存标题及自定义配置。先接受展示收益，不宣称完整交互事件或自动发送 readiness。
4. **完整 Signal 以身份通道为前提。** 拿到可验证的客户端/会话绑定及有序事件后，再适配进现有 observation/episode/attention 系统。首次连接快照不回放旧完成通知；断连先撤销覆盖；任何 Signal 都不新增执行授权。

涉及代码的后续实现仍须运行仓库规定的 gates、Signal Trace/census、隐私/EDR 检查，以及 installed-build Signal candidate 矩阵。原来的 `codex-daemon-refused` 应继续作为错误归属的安全回归；另加共享模式功能覆盖测试，不能用安全用例通过代替功能通过。

本次可接受的判断是：**现有安全模型不需要推翻，产品覆盖需要补；embedded 能较快恢复现有交互提示，共享模式有值得验证的终端展示来源，但 0.157.1 尚不能直接宣称完整 Signal 已解决。**

## 主要来源

- [官方 changelog](https://learn.chatgpt.com/docs/changelog)
- [官方 hooks 文档：trust、输入字段、transcript 限制](https://learn.chatgpt.com/docs/hooks)
- [官方 app-server 协议文档](https://learn.chatgpt.com/docs/app-server)
- [0.157.1 hook 环境快照](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/hooks/src/registry.rs)
- [0.157.1 hook 执行环境](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/hooks/src/engine/command_runner.rs)
- [0.157.1 daemon 启动排除条件](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/tui/src/daemon_startup.rs)
- [0.157.1 标题状态与 ID 截断](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/tui/src/chatwidget/status_surfaces.rs)
- [0.157.1 标题输出](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/tui/src/terminal_title.rs)
- Deck：`app/src-tauri/src/agent_status.rs`、`status-helper/src/main.rs`、`commands.rs`、`app/ui/js/pure.js`、`app/SMOKE.md`，及表内提交。
