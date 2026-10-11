# Pi v0.8.0 Decision Record

状态：设计完成，生产实现未开始。基线和源证据见[详细设计](README.md)与[调查证据](../../evidence/pi-investigation-20261012/README.md)。本记录锁定后续独立执行者可以决定和不可以决定的事项。

## ADR-01：分别验收可证明的能力

采用 P1a（working/turn-done）、P2（归属已验证的原生 Session 观察），保留原始P1全部三状态为BLOCKED，P3为BLOCKED。执行者必须继续完成不受阻的范围，不能因P3停止全部工作，也不能把P1a重命名为“P1完整PASS”。

原因：真实Pi有比旧hook更明确的settlement边界与可用进程拓扑；UI prompt当前API有已复现的源乱序；身份认证是另一层独立限制。把不同性质的限制合成一个“Pi不支持”会丢失可用价值；把它们都标PASS则误导用户。

拒绝：先实现三状态再补风险备注、只做命令名称识别、整体否定Pi、以P3negative test作为恢复功能通过。

## ADR-02：唯一生产传输为进程内Extension→现有UDS

选择A。不spawn status-helper，不开第二socket，不写home脚本，无后台Pi service。bundle资源随签名Deck发布，用户config只引用路径。

保持共享：origin、pane选择、foreground generation、Signal store/episode、Attention。新增Pi专属framing是对“没有native run ID、reload、故障撤销”的必要适配，不开放任意插件注册。

拒绝B：helper v2只允许source自己提供的原生UUID；Pi没有该ID。改helper既扩大其隐私payload契约又不能处理source事件乱序，还增加每事件spawn。拒绝C：wrapper、RPC/SDK常驻observer、sidecar代理、poll transcript、scrape TUI。它们没有更好的上游事实，增加执行责任或隐私面。

本轮原型是one-shot direct socket，仅验证拓扑；拟议persistent stream必须由执行者另行验证，不能继承原型PASS。

## ADR-03：观察者交互ID明确区别于原生ID

Pi身份层次：kernel `(pid,birth,foreground,tty,pane)`证明来源；backend stream lease识别一次连接；adapter run counter识别start到settled的观察区间；Pi native Session ID只用于P2。

不向native-v2塞随机UUID，不从Session ID、message entry、title、时间、cwd制造interaction。已有NativeUuid tracker语义不变；Pi使用显式typed key进入同一共享store。

源乱序与transport乱序不混淆：seq只能证明observer接收后的顺序，不能修复Pi把end先于start送给observer。

## ADR-04：当前禁止可信needs-input和automatic Pi delivery

UI原型证实较早Extension可让已消失的弹窗产生迟到start。内置/model也不发该事件。不能用计数器、kind/title、ctx.isIdle、时间窗、末状态或agent_settled作输入等待的权威快照。

生产P1a不订阅ui_prompt事件。显示有限覆盖；Pi目标自动行始终`pi-input-unverified`，包括已有first-send override行；不扩大remote admission。手动send now保持现有用户操作语义与target guards。

拒绝：为了“功能完整”重排用户Extension、包装/修改Pi runner、拦截工具作为输入批准、让其他Extension加入Deck私有协议。可向上游提出API需求，但本轮未获发送消息/发布issue授权，因此没有提交任何上游内容。

## ADR-05：P2观察与P3回执不可互换

当前`lifecycle.rs`只有armed事务mailbox，不是全天Session registry。P2增加同一模块归属的volatile当前观察，复用现有origin和连接，不复制Restart框架。

Claude grammar和final/resume reducers移到Claude adapter receipt contract；core保留全部proof/deadline/lease机制。Pi有观察能力没有ReceiptContract和ReentryCapability。source不能仅凭`reentry.is_some()`就通过Claude UUID parser。

拒绝：把所有native ID规范为UUID；把session_start当恢复确认；把shutdown(reload/new/resume/fork)当退出；把getSessionFile的路径存在性当认证；在磁盘持久化观察以便自动恢复。

## ADR-06：P3保持BLOCKED

已核验npm实际执行Node；所测Node有Developer ID，但JS不在其运行代码签名中。官方Bun Pi归档校验通过，实际可执行文件ad-hoc、identifier a.out、无Team ID。现有动态代码身份要求不满足。

拒绝：信任通用Node/Bun签名、路径/名称/版本、安装摘要名单、npm integrity、Session ID、用户“信任此文件”勾选；重打签名的Pi不能冒充供应方。

未来需要供应方可验证且绑定运行图像的独立分发或等强官方运行证明；还要解决Pi path不存在会创建新Session、CLI恢复start reason是startup、原生Session未落盘、绝对path launch grammar等前提。不能在本实现偷偷新增Pi launch函数等待日后打开。

## ADR-07：配置只保留一个权威副本

现有Pi settings的extensions路径选择三种薄entry variant之一：signal/lifecycle/both；共用同一个factory。两个开关从文件推导；不另存Deck enable booleans。只有资源在bundle里，home里只写配置。

这是三个常量入口而非三套长实现；相较额外sidecar配置/Deck状态/Pi状态三方同步更小。安装器只编辑自有array元素，保留用户byte、mode、symlink；malformed/冲突不修复成defaults；boot不重新启用用户显式禁用项。

拒绝：复制Extension到~/.pi、npm全局安装package、追加自定义配置状态文件、wrapper自动注入-e、强行卸载用户扩展。当前活跃Pi的reload由Pi自己决定，Deck不得自动打/reload。

## ADR-08：兼容门限与身份不同

初始API支持区间1.1.x，实际认证版本仅1.1.0。公开VERSION只用于UI/API兼容，不是可信产品认证；不spawn探测。旧Pi版本虽有部分事件但语义不同，先不给生产承诺。未来放宽区间需源码+runtime证据，不创建Reentry版本/摘要allowlist。

## ADR-09：native fixture与真实Agent执行分开

使用真实interactive Pi CLI和原生event循环，模型由隔离extension的deterministic provider响应；无需私人credential或外部模型。它证明TUI/事件/进程拓扑，不证明真实LLM正确性，也不是Deck生产E2E。

WK必须真实Tauri/WKWebView+production IPC，mock只能算layout。Claude/Codex现有回归gates仍要运行。测试资源用registry，实际HOME/Deck/Pi配置和用户tmux不动。

## 风险账本

| ID | 分类 | 结论/关闭条件 |
| --- | --- | --- |
| F-01 | 事实 | UI end→start反例；原始P1阻塞 |
| F-02 | 事实 | npm Node、Bun ad-hoc；P3阻塞 |
| F-03 | 事实 | session native ID支持非UUID；路径可指未创建文件；不能套Claude格式 |
| F-04 | 事实 | session_start startup不区分CLI首次与--session恢复 |
| I-01 | 推断 | direct persistent observer可保持短归属链；由one-shot原型支持，长连接需实现后验证 |
| U-01 | 未验证 | bundle资源安装/迁移+Pi真正加载+WK投影整链；执行者必须关闭 |
| U-02 | 未验证 | 断线/reload/同generation多observer/限流公平性；执行者必须关闭 |
| U-03 | 未验证 | Claude receipt contract提取后的行为等价性；完整test gate和native fixture验证 |
| U-04 | 未验证 | 1.1.x未实测patch、其他macOS/架构；不能从本机结果泛化 |
| R-01 | 残余 | same-pane进程可谎报presentation；无执行授权 |
| R-02 | 残余 | 用户未配置observer的间接node启动不可识别；不支持其自动Pi delivery |
| R-03 | 残余 | 外部Pi config不使用Deck锁，原子rename前极短竞争窗口；不能宣称任意并发编辑零丢失 |

## 执行者不得擅自决定

解除F-01/F-02、启用needs-input、扩大API兼容区间、信任任何新签名供应方、加入Pi remote admission或first-send override、持久化native ID/path、实现Pi自动launch、改CLAUDE.md、修改source-v2含义、弱化Codex/Claude origin、写home可执行代码、生产发布。发现新上游证据时提交具体设计差异供设计负责人审查，不能自作产品决定。

执行者可自主决定函数局部命名、在指定模块内部的必要拆分、测试fixture组织和性能调优；要保持本记录的语义和量化资源上限。任何范围内失败自行诊断/修复/回归，不要求用户做常规工程操作。

## 设计自审

已删除的候选：以helper合成UUID；以UI counter当可靠等待；以session_start当readiness；让Pi lifecycle进入Claude回执；让名称pi自动进入remote闭集。已选择有界单transport和共享store，未设计Agent执行框架。

独立专家只审查了事件乱序与settlement语义，未替代本设计者的源码调查、native实验和最终判断。审查认为UI mapping应BLOCKED；随后真实TUI反例证实。生产实现将由另一Agent承担；设计负责人不接管实现，返回报告后按acceptance.md独立复核。
