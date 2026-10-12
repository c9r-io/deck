# Pi v0.8.0 第一轮验收修复 Prompt

你是独立执行 Agent。本轮针对 7e4f8d7 的独立验收未通过。请在现有实现上确认根因、完成最小修复、自动运行验证，并提交完整证据，不能只调查后汇报。**所有进度说明、问题解释、最终报告和新增验收说明必须使用中文；代码符号、命令及项目既有英文文案可保留英文。**

设计和验收负责人没有接管产品实现；审查结果在 docs/evidence/pi-acceptance-20261012/README.md，机器证据在同目录 review-results.json。先核对当前 HEAD 和工作区，不覆盖负责人新增的审查文档，不把它们误判为自己的未提交改动。

## 先读约束

完整阅读 CLAUDE.md、AGENTS.md，再读 docs/design/pi-v0.8.0/{README.md,decisions.md,implementation-prompt.md,acceptance.md}、本 Follow-up、原实施证据 docs/evidence/pi-implementation-20261012/。读每个修改模块的头部契约，特别是 agent_pi、agent_status、agent_reentry/lifecycle、procinfo、scheduler、notify、Settings 和真实 WK 测试设施。

在本 Prompt 范围内，你可以自行实施、测试、诊断、修复设施、重复验证和作本地提交，不必逐步请示用户。你不是唯一工作者，保留他人的编辑。普通失败不得转交用户做 UI 点击、运行命令、配置或测试。不得改产品宪法、解除完整 P1/P3 阻塞、降低认证要求、扩大 remote admission、增加自动工具批准/Agent 调度、修改用户真实配置/数据、Push/Merge/Tag/Release 或发布任何内容。

保持唯一生产路径 Extension→既有 status.sock。Pi.reentry=None；不接入 Claude receipt mailbox；不订阅生产 ui_prompt 伪造 needs-input；所有已识别 Pi 自动发送仍 HOLD，所有 override 不得豁免。不得放宽 Claude/Codex 的身份和输入边界。

## F01 高优先级 修复跨 exec 的旧实例归属

事实：负责人用隔离 C peer，在 hello accepted、working ACK 1 后保持 PID/socket，exec 成另一根内可执行文件；新程序发送 seq=2 turn-done，生产后端仍 ACK 2，poll 显示 fg=exec-after 和 pi_signal=connected。具体结果见 review-results.json.same_pid_exec；原始脚本 /private/tmp/deck-pi-accept-n_kowcmb/exec_probe.py。该脚本依赖当时本地路径，复用时先转换为你本轮明确的隔离参数。

位置：agent_pi/transport.rs:401 verified；agent_status.rs 的 ForegroundGeneration 和 Pi generation evidence；transport/tests.rs:580。

根因：pid+birth 不等于 exec 实例；现有测试模拟的是 PID reuse。违反 A12 和旧 instance 隔离，不涉及把 P1 升级成供应方认证。原详细设计没有给出足以满足 A12 的 exec 原语，这也是设计缺口；本 Follow-up 明确补充连续性要求，不要求你假装原方案已经充分。

修复范围：在现有 procinfo/transport 归属链上补足可验证的执行实例连续性；hello 绑定、每帧 commit、reconcile 和 current evidence 读取都不能让旧 lease/旧证据在 exec 后继续代表原 observer。使用当前 macOS 官方可验证原语并通过实际实验验证。不能只比较进程自报名称、路径文本、Session ID、观察者自报 UUID；不能以通用 Node 签名当 Pi 产品认证。仅用于判断“是否仍是原运行实例”的事实，不得变成 P3 摘要白名单。读取失败安全撤销，并给闭合 unavailable 原因，不冒认另一个 pane。

至少测试真实同 PID 的不同 image exec、保留 FD 的迟到 frame、旧 callback、PID reuse、正常 Pi Node/Bun不误拒绝。明确区分 exec、磁盘文件替换和新进程 birth。不要把普通 parent/child fork 当 exec。若官方可用原语在既有权限/EDR边界内不能满足要求，给出最小反例和 BLOCKED 结论；不得擅自把 A12 改成 PASS 或批准新的残余。

## F02 高优先级 明示不支持的 Session ID 并保留一致性失败

位置：agent_pi/transport.rs:645–667、transport/tests.rs:1223、resources/pi/observer.ts:149、agent_reentry/lifecycle.rs 的 PiObserved/PiSession、pi-model 和对应文案。

事实：257 字节 ID 被普通 ACK 后直接 forget_pi；同一 lease 的下一条不同 ID snapshot 被当作首次记录接受。超过 observer 上限的 ID 被返回 null，连 unsupported 状态都没有。违反 B02 及同 lease 不得无 teardown 改 Session 的契约。

实现显式、闭合的 unsupported-native-id 降级，贯通 observer、wire/backend、session availability 与用户详情；日志不含 ID/path/正文。不得截断、hash 成假 ID、提高上限掩盖问题或仅补日志。

本次批准的最小语义：超出支持范围后，此 lease 的 lifecycle 观察进入显式不可用，清除当前可用 ID，但不能把“已无可信连续性”重置为“还没见过任何事件”；同 lease 不能用后续短 ID 重新取得正常 observed，新的合法 runtime/lease 才可重新观察。P1a 的独立 Signal 能力应保留。对于长到不能放进 wire 的 ID，observer 只发送不含 ID 的闭合不可用控制信息，不能先读后静默省略。非法语法仍按错误拒绝，不混同合法但超限。

测试边界 256、257、512、513；正常 A→超限→B、超限 start→短 snapshot、shutdown ID 不一致、新 lease 恢复、Signal-only 不访问 SessionManager、both 降级时 Signal 仍工作。修改目前把错误行为钉成 PASS 的测试，并增加真实 Pi/生产 IPC 可用性投影证据。

## F03 高优先级 两个能力必须能独立撤销

位置：transport.rs:303 enabled_changed、end、commit、mismatched；observer.ts 的 FINAL_REFUSALS/onLine；config status；pi-live-smoke.mjs:224。

事实：both→signal 或 both→lifecycle 都结束整个 stream；observer 收到 capability-disabled 后直到 reload 不再连接。仍开启的能力也失效，signal 的未读 episode 被删除。现有 WK checkpoint 只证明开关值，漏掉剩余能力继续工作。违反 P2 不破坏 P1 和独立 opt-in。

采用现有 stream 内的能力收紧，保留未撤销能力的 lease/运行连续性。允许增加必要的闭合撤销回复与当前有效 capability mask，不能增加第二个 listener、第二套连接或另存配置布尔值。区分 runtime 加载的能力与本 lease 仍获准使用的能力；只允许收紧，后续重新打开能力仍按既有 reload 生效规则，不用 ping/ACK 自动扩权。

关闭 Signal 时只清本 lease 的 Signal/episode；关闭 lifecycle 时只清其 Session 观察。把收紧及时告知 observer，停止被撤销能力的读取和新帧；已在途帧须保持序号有界、一致处理，绝不能在收紧后写回已禁用状态，也不能以它们为由中断仍启用的能力。原本未授予的 capability 伪造仍拒绝。双关闭结束连接。若配置读取失败则按现有 fail-closed 契约处理。

真实 Node/Bun 分别验证 idle 与 active 中 both→signal、both→lifecycle、单关闭/双关闭、连续切换、在途 frame 和重新启用等待 reload。断言剩余能力实际产出、旧禁用能力无写入、未读状态不误删、UI current/needs-reload 与 runtime 一致。尤其给现有 pi-session checkpoint 加“关 lifecycle 后继续完成一轮 Signal”的生产断言，不能只看 status.pi=true。

## F04 中优先级 完成真实可见 UI 门禁

对应 G06、C03、B12、A26。真实打开 Settings 和 Restart Review 对话框；现有 preview IPC 加 model 计算不算实际渲染。自动运行中英文、1280×800、支持的 font scales、长标题、Pi/Claude/Codex/shell 混合、noCandidate/failure 布局，并保存截图及溢出/遮挡等检查结果。mock/layout 与 production WK 分列，不冒充彼此。

真实 Pi 场景要看到 coverage、lifecycle unavailable 原因、两个开关及 Pi 不可选原因。混合 Claude 的视觉测试可用既有 fixture，但必须标注；没有账户的真实 Claude 重入仍单列 BLOCKED，不借用用户账户。系统通知权限不足亦单列，不改 TCC、不装系统服务。

优先复用已有隔离 smoke；本机屏幕锁定时不要盲目反复开窗。使用 Mac mini 前实时 inventory；保持已安装 Deck 原状，全路径 tmux socket、独立 HOME/data/session。截图、鼠标、键盘由你自动完成，不要求用户协助。若渲染发现问题，在已批准 UX 范围修复并重跑。

## F05 中优先级 让证据对应可复核的最终构建

事实：原 native-result 记的 deck_head 为 812b786，报告却称最终 tree；缺少当时测试 binary hash 和 dirty diff；WK 摘要亦不足以独立锁定 carrier。不是认定你造假或测试旧代码，而是证据不足。

最终修复完成后重新生成必要证据，记录准确代码 commit、dirty 状态及必要 source diff/hash、test executable hash、WK carrier manifest、observer hashes、runtime 版本/hash、实际命令、exit code、每场景结果与 cleanup。文档晚于代码提交可以，但必须指出被测代码与文档提交的关系。保留完整私有日志，提交脱敏摘要，不能泄漏原生 Session ID。

改正 A12“synthetic exec PASS”和 B02“静默丢弃 PASS”等错误映射。构建后不得修改资源再复用旧 embedded UI/binary 结果。报告 Unit/Integration/Native/WK/Layout/Safety 各自覆盖、FAIL/BLOCKED/UNRUN，不写“所有测试通过”掩盖未运行门禁。

## F06 高优先级 共享 listener 的首行准入必须有界

这是基线已存在但此次设计依赖、尚未满足的安全可用性条件，不得写成 Pi 新引入的回归。位置 agent_status.rs:1454 read_first_line 与串行 handle_stream/listener；Pi transport 上限在此之后才起作用。

独立反例：慢连接每 0.5 秒发送一个空格，另一探针 3.53 秒得不到处理，慢连接关闭后立即得到拒绝。两秒是每次 read 的 timeout，不能证明整条首行两秒内结束。A24/设计要求不能让一个坏客户端占住所有协议的 accept loop。

限定批准范围：在既有 listener 内补足有界的首行读取/分发，复用现有异步设施；每连接绝对首行期限、明确的 pre-hello 并发上限（不超过 32）、有界内存和有界拒绝。不能新增监听 socket、每连接 OS 线程、无限任务或通用插件运行时。至少保证有剩余容量时，一个静默/滴流 peer 不阻止其他 peer 的分发；满时有界拒绝，不宣称无限负载下必定接收。保留 Claude/Codex helper 的 close/取证顺序和 lifecycle receipt 的存活取证，不以把 helper 提前释放为代价通过测试。

验证真实 socket 上的 silent、drip、partial line、超长首行、连接/队列满、正常 Pi heartbeat/Signal，以及同场 Claude/Codex/receipt 分发；用明确时间上界和实际处理记录。首行如果读取到后续字节，必须保留 remainder 给 stream parser 或明确拒绝不合法 pipelining，不能悄悄消费后丢弃。Pi 正式 observer 仍等待 accepted 才发帧。

## 顺序与验证要求

先锁定失败回归：F01、F02、F03、F06；在其各自模块做最小修复，再做 UI 投影/F04，最后 F05 固化最终产物。不要重写已通过的 Claude receipt reducers 或另建 Pi lifecycle/restart 框架。模块内局部拆分可自行决定。

必跑 fmt、CI 形式的 workspace all-targets/all-features clippy、workspace tests/仓库覆盖门禁、frontend syntax/ui-tests/check、edr_quiet/log_privacy/signal_census/external_admission、sidecar/resource provenance；构建和资源涉及的发布扫描按既有规则跑。额外 CI 项的适用性如实说明，不擅自跳过适用门禁。

真实 Pi npm/Node 与 standalone/Bun 两种版本均跑 Signal 主矩阵、新的能力撤销、exec 负例和 lifecycle 新反例；复用已有驱动，保持 session/new/fork/resume/reload/shutdown 的语义证据，不把 native fixture 当 WK。P2 的 standalone 运行中 session 切换尚未实测，利用现有 provider 自动补齐或给出具体阻塞。重新跑真实 production WK functional smoke 和 F04 layout。进行最终完整 diff 审查，不只审测试结果。

所有测试资源在独立临时根内；登记后才进行测试，以 experiment registry 为唯一停止途径。registry 拒绝必须诊断，不能以任意 PID kill、pkill 或 tmux -L 清理绕过。不修改真实 Pi/Deck 配置，不安装全局工具，不改变系统安全设置。只保留必要证据，先核实没有 owned process/socket，再处理精确的测试根；不要擅自清理用户或其他任务的 Trash。

## 完成标准与中文报告

逐项 F01–F06 报告根因、修复 commit/文件、测试命令及 exit、期望与实测、证据位置、剩余风险。每一项必须 PASS、FAIL 或有证据的 BLOCKED。BLOCKED 只用于确实缺少满足契约的上游/系统原语或环境条件；普通代码失败、第一次设施失败、缺少用户点击不是阻塞理由。遇阻仍完成独立不受影响的项。

本地提交获准，保留易审查的范围；不要 push、merge、tag、release。最终中文报告列出 Design、Implementation、P1a、完整 P1、P2、P3、Security、Regression、Native E2E、Product Constitution 判定，以及清理结果、未运行项和剩余阻塞。不要把拒绝 P3 的测试通过写成 P3 PASS。完成后交回独立验收，不能由你自签最终通过。
