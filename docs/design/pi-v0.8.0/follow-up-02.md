# Pi v0.8.0 第二轮验收修复 Prompt

你是独立执行 Agent。请在现有实现上确认根因、完成下面限定范围的修复、自动验证并提交证据。不要只调查后汇报。**进度说明、设计说明、问题解释、最终报告及新增验收文档全部使用中文；代码符号、命令和产品既有英文文案可保留英文。**

本轮验收对象：代码 `eb16c71`，证据提交 `546eb45`。负责人独立复验后，F02–F06 在各自范围内通过；F01 未通过。P1a 和 P2 的正常功能已有真实 Pi 证据，但共同依赖的发送者归属仍失败，不能签署阶段 PASS。完整 P1 与 P3 保持原来的 BLOCKED。负责人没有修改生产代码。

先读 `CLAUDE.md`、`AGENTS.md`、`docs/design/pi-v0.8.0/{README.md,decisions.md,acceptance.md,implementation-prompt.md,follow-up-01.md}`，以及 `docs/evidence/pi-followup-01-acceptance-20261012/README.md` 和执行证据 `docs/evidence/pi-followup-01-20261012/`。修改前阅读 owning module 头部，重点是 `agent_pi/transport.rs`、`agent_status.rs`、`procinfo.rs`、observer 与 native harness。

在这些约束内，你可以自主调查、实现、修复测试设施、运行全部必要测试、清理和本地提交，不必逐步征求用户许可。你不是唯一工作者，保留他人的改动，包括本次验收文档。不改产品宪法，不扩大功能范围，不降低身份要求，不操作用户真实配置或会话，不安装全局工具，不修改 TCC/系统设置，不 Push/Merge/Tag/Release。

## R01 高：继承 FD 的后台子进程可冒用父 lease

**事实与证据。** 负责人在生产 listener/transport/store/poll 上运行隔离 C peer：前台父进程完成 hello 与 working，随后 fork；子进程 `setsid()`，PID/PGID/SID 与父不同，`ps` 显示无控制终端。父保持前台且未 exec。子进程使用继承连接发送 seq=2 `turn-done` 和 seq=3 lifecycle `snapshot`，分别得到 ACK 2、ACK 3；poll 同时显示 `turn-done`、`pi_signal=connected`、`pi_session=observed`。见验收目录 `inherited-fd-both.json`、`inherited-fd-probe.py`。测试中的 Session ID 是专门构造的测试值。

**位置与根因。** `transport.rs` 的 `Service::hand` 缓存 peer/watch；`committed` / `verified`（本轮基线约 529、745、938 行）检查的仍是父进程。父 watch 没有 exec/exit，并不证明这一帧由父进程写出。原设计要求发送者为 foreground leader，并明确排除 helper、后台工具和子进程。一般的“同一进程内 Extension 自报有误”残余不包括另一个进程通过继承连接冒用父 lease。执行报告 §3 F01 剩余风险第 2 条不能视为已经批准。

**要求。** 在唯一生产路径 Extension → 现有 status socket 中，补足当前帧实际发送实例到 lease / foreground / ancestry / terminal 的绑定。不能把“缓存的父进程仍正常”当成发送者证明，也不能仅以正式 observer 通常不传递 FD、设置 CLOEXEC、父子同用户或同项目来关闭问题。CLOEXEC 不阻止纯 fork 的继承。

先验证拟用原语能证明什么。现有实验显示 `LOCAL_PEERPID` / `LOCAL_PEERTOKEN` 会随使用连接的进程变化；这不意味着一次查询就能证明缓冲区中**每一帧**的来源。必须覆盖 child 写帧后 parent 心跳/读写改变该值、拆行、合并读取、排队后延迟 commit 等顺序。不得把“最后使用者”误称为“这段字节的作者”。可以在现有模块内修改必要的闭合握手/协议；不要同时保留两条生产路径，不添加通用插件运行时、helper 子进程或新的 Agent 执行能力。

无法证明来源时，该帧不得改变 Signal、episode、lifecycle 记录或可信 coverage；处理 lease 失效与 unavailable 必须明确，并保留已有自动输入安全边界。正常 Pi 的工具子进程创建不能一概被当作 observer exec，不能靠禁用工具或结束所有 fork 来通过。

**必要测试。** 前台父进程 + 同进程组 child、`setsid` child、继承 FD 后 child exec、child 写后 parent 正常心跳、拆帧/合并帧；Signal、lifecycle、reset 分别验证。正常父进程发帧、Node/Bun 正常工具子进程、两个 pane 互不影响。所有拒绝均断言没有中途出现再被清掉的假状态，而非只检查最终空状态。

## R02 高：watch 建立前的旧 hello / 缓冲帧可归给新映像

**事实与推断分开。** 代码在首行已读完并进入 dispatcher 后，才于 `Service::hand` 读取 peer、建立 watch；`rest` 作为后续帧提交。执行报告 §3 F01 剩余风险第 1 条也承认写 hello 与登记 watch 之间的 exec 不会被报告。负责人没有把该精确时序报告成已运行的 native 反例；此项依据源码与已承认的原语边界成立。

**根因。** kqueue 只证明登记以后的连续性。旧实例可先把 hello，乃至 hello 后的状态写入内核缓冲，再同 PID exec。之后才建立的 watch 会把新映像看成 Same，但它不能为之前的字节作归属证明。影响不仅是版本/能力声明，因为当前 parser 明确接受 `rest` 中的帧。即使新映像有能力自己发一个新 hello，也不能把它从未发过的旧状态当作新状态。

**要求。** 证明完整的建联边界：被接受的声明及其后帧确实属于所绑定的执行实例。可使用必要的闭合握手与经验证的内核事实，但不能用 observer 自报 UUID、PID/birth、路径或 Session ID 代替证明。仅把 watch 移到读首行前还不充分：旧字节可能在 accept/watch 之前已进入内核缓冲。若使用服务端新鲜挑战，明确它阻断的是预先缓冲的旧字节，仍须独立解决 R01 的实际写者问题；不要把继承的 nonce 当进程身份。

保持既有 watch 建立后的 exec/exit 撤销、每帧 commit 与所有 current-evidence 读取检查。普通 fork、child exec、磁盘文件替换不能误判为父实例 exec；内核不可问仍 fail-closed。评估同 PID 多 watch 登记时是否会遗漏并发 exec，结论须有原语语义或定向实验证据，不凭猜测。

**必要测试。** 旧 hello 已缓冲再 exec；旧 hello + working/snapshot 已缓冲再 exec；hello 分片期间 exec；登记 watch 附近 exec；接受后真实同 PID exec；新映像沉默。通过测试设施控制先后关系，不修改生产行为制造成功。断言旧实例的 frame、Session、coverage 不会在新映像出现；保持正常 Pi 的启动、重连、reload。

## R03 中：native harness 会漏读 libtest 前缀后的事件

负责人直接运行原生驱动时，44 项通过后第二个后端等待 ready 超时，exit 1；当时后端仍在处理 Pi 心跳，最终 registry 清理 exit 0。`Tap.read`（`scripts/pi_native.py:130` 附近）只接受以 `@tap ` 开头的行；Rust libtest 的 `test agent_pi::tests::native::tap ... ` 与首条事件可在同一行。独立重跑的 raw stdout 已记录这种前缀；首次超时没有保存 raw stdout，因此“该次丢的是 ready”是有根据的根因判断，不能冒充已直接抓到的字节证据。

负责人只在临时驱动副本中识别该确切前缀并保留所有 raw stdout，没有修改产品或仓库测试。相同测试可执行文件随后完成 170/170 个定向 native 检查，exit 0。

请确认并最小修复正式测试设施的 framing：不能依靠线程调度决定首条事件是否被读到；保留可诊断的原始输出。可处理确切的 libtest 前缀或采用明确的测试输出边界，不应从任意日志文本里猜 JSON。不增加超时掩盖漏读，不忽略解析错误、不放松产品断言。测试 ready/now 各自为首条、正常行、异常行和 restart，并重新运行 native 矩阵。首次失败与设施修复分列。

## 本轮设计裁决与禁止回退

以下裁决以当前未发布的 Pi 集成为边界：

- 同意使用 kqueue `NOTE_EXEC|NOTE_EXIT` 作为登记后的连续性事实；它不是 Pi 供应方认证，也没有解决 R01/R02。
- 同意新增闭合的 `unavailable`、`capabilities`、`revoked`，以及先通知收紧再 `accepted`。本次未发布 wire v1 的调整可以保留；若再次改变握手，必须明确版本和旧 runtime 安全失效/重载行为，不可默默混用。
- 同意 hello 能力与配置取交集，且本 lease 只收紧、不扩权；重新启用仍需要 reload。保留 F02 的 unsupported-native-id 粘性与不泄露 ID，保留 F03 的独立撤销。
- 同意 `needs-reload` 文案覆盖 observer 已结束而 Pi 仍在运行，以及新增 unsupported 文案；可用性描述不得冒充已连接。
- 同意 F06 的 32 个首行槽位、绝对期限、有界 dispatcher 队列、一个固定线程及闭合拒绝原因。完成首行的顺序取代 accept 顺序可以接受，已有 hook/receipt 取证和存活顺序不得退回。
- 已知 exec 后清除旧 Pi 证明符合旧证据不得沿用的规则；内核不可问时不宣称 connected。不要利用本轮变更解除已识别 Pi 的自动输入 HOLD。
- `PI.reentry=None`，不接 Claude receipt mailbox，不启动精确重入，不伪造 needs-input，不扩展 scheduler/remote 授权。P3 的拒绝测试通过仍不是 P3 PASS。

## 顺序、验证与 BLOCKED

先把 R01 的独立反例和 R02 的确定时序钉成失败测试；完成发送者与建联边界的一套实现，再修 R03，最后固化最终构建证据。优先局部最小修复，不重写已通过的 lifecycle reducer、Settings 或 Claude/Codex。负责人已通过的 F02–F06 要做相关回归，不能作为可以重写的空白。

自主执行仓库适用的 fmt、clippy、CI 形式 workspace 测试/覆盖率、安全与隐私扫描、UI/脚本门禁和构建资源扫描。运行完整真实 Pi npm/Node 与 standalone/Bun native 矩阵，并加入上述反例。两种真实 Pi 的 production WK functional smoke 在最终载体重新跑；若 UI/CSS/文案未变，可引用 F04 的布局证据并列明内容哈希与复用范围；如有可见变化则重跑对应中英文/字体/最小窗口布局。账户、系统通知、macOS 14 和签名载体的限制如实分列，不把未运行改写为 PASS。

测试只用独立短临时根、HOME、Pi 配置/Session、tmux socket 与 Deck data；按 AGENTS 的 experiment registry 登记和停止。失败后自行诊断，禁止任意 PID kill、pkill、tmux -L 清理或触碰用户现有服务。保留必要日志，不清空用户或前轮测试的废纸篓。

若在现有权限、公开原语和 EDR 边界内确实无法证明 R01/R02，则提交已排除方案、可重复的最小反例和最小设计变更建议，明确 P1a/P2 因共享身份前提 BLOCKED。不要通过放宽为“同用户/同 pane 可冒用”、仅比较路径/名称、加大超时或改测试预期来取得 PASS。普通代码或设施故障不是 BLOCKED。任何超出唯一 IPC 路径/产品职责/安全契约的变更先作为设计问题交回，不擅自实施。

最终中文报告按 R01/R02/R03 给根因、commit/文件、期望与实测、命令/exit、产物哈希、清理和剩余风险；明确区分真实 Pi、C adversarial fixture、unit、WK 与 layout。记录被测代码 commit、dirty 状态、源文件/test binary/observer/carrier 哈希，证据提交可以晚于代码但须证明源文件未变。保留失败记录。逐项给 Design、Implementation、P1a、完整 P1、P2、P3、Security、Regression、Native E2E、Product Constitution 的 PASS/FAIL/BLOCKED，自评不能代替独立验收。
