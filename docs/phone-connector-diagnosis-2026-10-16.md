# Phone Connector：诊断与修复方案（2026-10-16）

本文记录最初诊断与方案设计。用户随后授权实施，并要求完成隔离验收；修复与验收结果见[后续证据](evidence/phone-connector-fix-20261016/README.md)。用户最主要的反馈是：手机无法取得正确的卡片列表，进入卡片后无法取得正确内容，整体难用。

## 结论

现有 Connector 把“哪些卡片允许远程输入”的严格准入规则，同时当成了“手机应该看见哪些工作”的规则；客户端又把一次性终端截图放在长表单底部，缺少可靠刷新与新鲜度提示。这两个问题共同破坏了用户对“同一个 Deck 看板及其当前内容”的预期。只调整错误提示或视觉样式不能解决。

建议先修复同步、身份与内容呈现，再明确手机看板的共享范围。手机应帮助用户找到需要关注的工作、了解现状并作出一次明确操作；不应要求用户理解 generation、operation ID、原始请求体或恢复日志。

## 基线与证据边界

- 源码基线 `a326cdf3a6b3f1b18176da9079659e4762408eca`，源码版本 0.8.2；本机已安装桌面版为 0.8.1。测试当前源码不能冒充真机已安装版本验证。
- 使用外置 Xcode 27.1（27A9275）、现有 iOS 27.0 runtime、一次性 iPhone 17 Simulator。Xcode 27 的可视模拟器入口是 [Device Hub](https://developer.apple.com/documentation/xcode/managing-your-simulated-and-physical-devices-in-device-hub)。
- 生产数据只读统计：本机持久化 Board 有 5 张卡片，5 张的保存命令均为空；按当前源码准入规则，0 张可投影到手机。没有读取、展示其终端内容，没有修改卡片或现有配对。没有核实用户手机配对的 host identity，因此这是高度相关的本机证据，不是对真机现场的唯一归因。
- 使用新建的隔离桌面 smoke host、loopback pinned HTTPS 与一次性配对；测试证书仅存在 smoke 进程内。测试 tmux 使用实验根目录内的完整 socket，进程与 server 登记后通过 registry 回收。
- 旧诊断入口的 Keychain 静态检查已落后于代码：当前隔离分支是 `cfg!(test) || smoke_faults::enabled()`；其按 socket 名清理的方式也落后于最新实验规则。本轮在 `/tmp` 副本中校准检查并换成 registry，未改变产品源码或放宽测试断言。
- 保留用户已有的 `first-party-governance-20261016` 和 `research_notes/` 改动。

运行结果与截图见 [证据目录](evidence/phone-connector-ux-20261016/README.md)。串行模拟器 UI 基线为 3 passed、0 failed、0 skipped：真实 Codex 输出、三轮停止会话及前后台/重启、Agent 回到 shell 后拒绝读写。Swift Core 为 61 passed、1 按设计跳过的 opt-in host 测试。首轮并行 UI 失败保留，未作为现场根因；串行 app-hosted 复核 2 passed、0 failed、0 skipped，覆盖真实 host 的便签操作/CAS/恢复与 Keychain 生命周期。首轮保存超时未在串行复核复现，保留失败记录，不能视为已修复。清理结论详见证据记录。

## 主要问题与根因

| 优先级 | 问题 | 证据与根因 | 对用户的影响 |
| --- | --- | --- | --- |
| P0 | 手机卡片列表与桌面预期不一致，且不解释缺失 | `connector/projection.rs::snapshot_in` 在探测前按保存的 `cmd` 过滤；仅允许裸 `codex`/`claude` 加受限简单参数。空命令、shell 内手动启动 Agent、Pi、路径或环境前缀都不能成为手机目标。本机 5/5 保存命令为空。 | 用户把“被策略过滤”理解成同步坏了；刷新多少次都不会出现。 |
| P0 | 已打开内容没有可靠更新路径和新鲜度标识（代码确认） | `DeckConnectorApp` 前台事件只调用 `model.refresh()`；`AppModel.refresh()` 更新 snapshot、草稿和 journal，不更新 outputs/buffers；详情 `.task` 在出现时加载，没有按当前目标或前台事件重新加载。UI 不显示已有 `capturedAt`。 | 返回前台后列表可能是新的、正文却是旧的；用户无法判断是否还在看当前工作。 |
| P0 | 主内容埋在笔记和发送表单下面 | `TaskDetailView` 的顺序是 scratchpad → composer → output。已有笔记全部铺开，再放输入框、权限解释，最后才是固定 260pt 的嵌套终端滚动区。 | 最常见的“点卡片看进展”变成寻找内容；笔记越多问题越大。 |
| P1 | “最新输出”并不刷新 | `TerminalOutputSnapshotView` 的按钮只调用 `scrollTo`。显示成功输出时没有同等显著的刷新按钮，只能下拉整个长列表。 | 点了后仍看到旧文本，很像内容获取故障。 |
| P1 | 列表缺少桌面上下文和注意力语义 | wire 已有 `columnId` 和 columns，但 iOS 只按 project 铺平；status 只有 probe 的 running/stopped/unknown，不是 Agent Signal。 | 分不清工作放在哪一列、哪项需要人；running 不能代表 Agent 正在工作，更不能代表不需要关注。 |
| P1 | 恢复阻塞读取，正常等待要求用户手动追问 | `refresh()` 先串行 recover 所有未决操作，再取 snapshot；详情刷新又经 `checkOriginalOperations()` 间接 refresh 后再次 refresh。网络请求 resource timeout 为 45 秒，多个未决项会放大等待。 | 操作结果慢或断线时，卡片列表也被拖住；恢复入口暴露原始 operation ID，增加人工检查。 |
| P1 | 内容请求缺少卡片级时序栅栏 | outputs 以 card ID 缓存，`loadOutput` 只检查配对 epoch/client/journal，未核验当前卡片 generation 或同卡请求序号；HTTPClient 验证输出结构但不比较响应 cardId 与请求 cardId。 | 代码层面存在旧请求覆盖新请求、重启后短暂展示旧内容的风险；本轮不把未执行的乱序网络场景记为已复现。 |
| P1 | 刷新后卡片消失时，导航可能变为空白 | `navigationDestination` 在 snapshot 找不到 ID 时直接没有内容；内层 TaskDetailView 的“Task unavailable”分支因此未必有机会呈现。属于代码确认、待专项 UI 复现。 | 删除、过滤范围改变或快照变化后，用户可能只见空白详情，而非明确解释。 |
| P1 | 未保存便签与消息草稿的耐久性不一致（代码确认） | 消息有 journal/coordinator；newNote 和编辑 sheet 的 text 仅为 SwiftUI `@State`，没有离开确认或持久化。 | 返回列表、关闭编辑或进程退出可能丢掉尚未保存的文字。 |
| P2 | 中文与英文混杂，内部概念占据主界面 | catalog 只有 33 个业务键；Tasks、Queue、配对说明、许多错误和操作说明不在该 catalog。恢复操作直接显示 generation、原始 ID/body。 | 用户需要读实现说明才能完成普通操作。 |

“输出不可用”必须继续区分原因：没有可读会话、前台不是受支持 Agent、读取期间目标变化、实际读取失败、权限/控制权不允许。历史上统一 `unavailable` 的问题已修复，不能将旧报告的缺陷直接当成本轮新发现。

停止会话的原因不一致已在[模拟器截图](evidence/phone-connector-ux-20261016/stopped-detail.png)观察到：发送区显示无法确认目标，输出区显示没有可读会话。源码上，snapshot 探测没有复用 output 路径对缺失 pane 的精确补查，`unknown` 因而继续传播到发送区。

另有一项代码级风险需要专项复现：snapshot 的 `canSend` 没有合并 MCP 控制权 fence，可能呈现可操作但读取/发送随后被拒绝；它不构成绕过后端权限的证据。

## 产品宪法约束下的方案

依据 [CLAUDE.md](../CLAUDE.md) 的产品北极星：减少反复检查与重建上下文；呈现可行动的真实状态；Observation、Authority、Readiness 分离；Deck 管理跨工作的注意力，Agent 管理工作本身。

### 1. 列表：把看板身份与操作权限分开

短期在现有共享范围内说明“仅显示已配置远程访问的 Codex/Claude 卡片”，提供清晰空状态、配对主机名、最近同步时间和错误状态。桌面对应位置说明每张卡片为何不可见，避免要求用户靠猜命令拼写解决。

目标方案是手机展示**用户授权共享的 Board 概要**，并对每张卡片分别提供可读输出、可发消息、可编辑笔记等能力与拒绝原因。保留项目、列、稳定 card ID 和用户放置顺序；列表过滤/排序是视图，不移动 Board 卡片。禁止把 shell 存活的 running 解释成 Agent 工作状态。

共享普通 shell 卡片的标题/项目等元数据也扩大了当前 Connector 的读取范围，必须作为明确的产品/授权变更：在桌面共享设置中取得本机授权并有兼容方案。此方案不意味着自动允许 shell 输出或输入。对于 shell 内手动启动的 Agent，未来如要支持远程操作，应通过单独审查的显式目标绑定与当前进程身份检查；不能因为看见 Agent 名称就取得权限。

需要关注的视图只消费已有可信 Signal/Attention 事实：needs-input 可行动，turn-done 是交互边界，不是任务成功或结束；不可归属的 Signal 显示覆盖不足，不制造 Agent 状态。保留查看全部已共享工作的入口。

### 2. 详情：先看到当前内容，再采取动作

建议默认结构：卡片标题 + 项目/列 → 连接/读取状态与采集时间 → 可展开的当前终端快照 → 底部消息输入；笔记与队列放在清晰的次级入口。默认首屏应出现正文或可解释的不可读状态。

终端内容继续明确是有界快照，不伪装成完整聊天记录。提供“刷新内容”和“跳到末尾”两个准确命名的动作；长行支持适合阅读的换行模式与原始等宽模式。正文滚动与外层页面滚动避免争抢手势。阅读旧段落时，新输出只提示“有新输出”，不要强制拉回底部。

消息输入保持单一主动作“发送”；“存为便签”是明确的另一个动作，不能把自动保存消息草稿误称为已存到桌面。键盘展开时仍可看到目标卡片与发送结果。便签编辑草稿按 host/device/card/entry 保存，复用耐久草稿机制；新目标不能继承旧目标的发送授权。

### 3. 同步：有限、自动、可解释

- 把 snapshot/detail 读取与 journal 结果恢复拆开调度，未决命令不能挡住列表读取；合并重复刷新，每张卡片只保留一条当前详情读取链。
- 打开详情、返回前台、用户刷新都刷新当前卡片内容；前台可见时做有界的周期读取或现有能力支持的更新，退后台取消。节奏与耗电由实测决定，不承诺后台实时通知。
- 响应必须匹配 host/device、cardId、当前 generation 和该卡最新请求序号。晚到旧响应直接丢弃；目标变化时不得继续把旧输出标成当前内容。
- 区分“首次加载、已更新、正在更新、旧快照/离线、权限不允许、目标变化、真实失败”。网络故障可保留明确标旧的已授权内容；撤权、host 切换和目标身份变化须遵守清除/隔离边界。
- 短暂未决时自动 GET 同一个 operation ID，有限退避；已确认结果自动解锁。真正 ambiguous/expired 不重发，不偷偷生成新 ID。只有主机证实不存在，才提供原 ID/seq/body 的明确重试；不把等待或成功读取当成发送权限或 readiness。
- 正常文案用“正在确认是否送达”“尚未送达，可重试”“结果无法确认，请在 Mac 核实”；原始 ID、错误码和阶段放在可复制的诊断详情里。失败不能靠消除提示伪装为流畅。

### 4. 保留安全边界，修正呈现边界

沿用 pinned TLS、设备撤销、MCP 控制权、前后 foreground/generation 检查、immutable operation ID/seq、CAS revision、发送前持久化和唯一 Board writer。保留手机消息与本机批准 preset 的权限差异；新增手机视图不是授权自动继续步骤的理由。

用统一的 capability/reason 投影向手机解释限制，读、写、排队各自准确；未知能力 fail closed。不要让 UI 的判断替代后端 admission，也不要用输出文本解析去重建 Agent 任务语义。

## 实施顺序与验收

| 阶段 | 交付 | 验收条件 |
| --- | --- | --- |
| A：恢复可信度 | 内容刷新链、请求时序防护、采集时间、真实错误状态、首屏输出、准确刷新按钮；现有列表范围与空状态说明 | 打开即可看到正文/准确原因；前后台变化后能取到新标记；旧响应不能覆盖新 generation；断线不伪装在线；同一操作不重复发送。 |
| B：解决列表预期 | 经本机授权的共享范围、完整身份/项目/列、能力与拒绝原因、现有可信注意力投影 | 建立桌面 Board 与手机概要逐卡对照矩阵：支持 Agent、空命令、手动启动、停止、Pi、MCP 控制、归档/移除、同名卡片。每项出现/隐藏均可解释；展示元数据不扩大读写权限。 |
| C：减少操作负担 | 独立笔记入口、统一草稿耐久性、中文文案、有限自动确认、键盘/长文本/大字体体验 | 返回列表与进程重启保留草稿；保存/送达/等待不混淆；无须理解内部术语；中文、深色、大字体、小屏和横屏可读可操作。 |

实现归属：host projection/capability 在 `app/src-tauri/src/connector/`，共享准入仍由 `admission.rs` 负责；同步/恢复在 iOS `AppModel` 与 `CommandJournal`，布局在 `Views.swift`，生命周期在 `DeckConnectorApp.swift`。协议字段变化同步更新 Rust/Swift golden fixtures，保持旧 host/client 明确降级；未知字段不能授予新能力。

实施时运行相关 Swift Core、app-hosted XCTest、真实 host Simulator E2E 和协议 Rust 测试；涉及生产代码时按 CLAUDE 的前后端 gate 完整验证。真机另验扫码、相机拒绝、Local Network 权限、Wi-Fi/VPN、Mac 睡眠、锁屏 Keychain 与后台回收。模拟器 loopback 成功不能替代这些测试。

建议增加能使缺陷真实失败的场景：先显示 A 输出，后台改变为 B，回前台必须显示 B 的内容与采集时间；同卡 A/B generation 请求反序完成；多笔 pending 时 snapshot 仍能到达；已接受 POST 的响应丢失后仅查询原操作并最终恰好产生一次结果；新建/编辑便签未保存离页与重启；可读变不可读时不暴露 shell 或旧目标输出。
