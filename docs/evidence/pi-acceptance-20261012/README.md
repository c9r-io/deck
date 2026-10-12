# Pi v0.8.0 独立验收

审查基线 1bdcf57，执行者最终提交 7e4f8d7。结论：本轮实现不予验收，需完成 [中文 Follow-up](../../design/pi-v0.8.0/follow-up-01.md)。完整 P1 和 P3 的上游阻塞仍保留，不要求执行者通过降低标准解除。

## 判定

| 项目 | 判定 | 范围与依据 |
| --- | --- | --- |
| Design Verdict | FAIL | 原详细设计的 pid/birth 检查不足以兑现 A12 的 exec 要求，需补足运行实例连续性；静态 Adapter、单一 UDS、权限边界及阶段划分保留 |
| Implementation Verdict | FAIL | F01 至 F06 未关闭 |
| P1a Signal Verdict | FAIL | same-PID exec 后旧 lease 可继续写 Signal；能力关闭相互影响；入口有界性和 UI 门禁未完成 |
| P1 Full Signal Verdict | BLOCKED | 已调查的 Pi 1.1.0 UI 事件缺可靠顺序和等待快照；未新增上游解除证据 |
| P2 Lifecycle Verdict | FAIL | 超长 ID 静默丢弃，并可清空一致性记录；两个能力不能独立撤销；受 F01 影响 |
| P3 Re-entry Verdict | BLOCKED | 未证明 Pi 供应方运行代码身份。当前拒绝路径与 PI.reentry=None 符合设计 |
| Security Verdict | FAIL | 已复现注意力来源实例连续性缺口；未发现此次缺口授予自动输入或 Pi 重入权限 |
| Regression Verdict | PASS | 独立重跑 1209 单元测试、44 安全扫描/回归通过，13 项既有 ignored；仅限所列范围，非带账户全量 live/所有 CI |
| Native E2E Verdict | FAIL | 完整验收矩阵未关闭；本轮真实 Pi 子集 71/71 PASS，执行者 WK 功能证据与缺失的布局证据分开计 |
| Product Constitution Verdict | FAIL | 原生 ID 的静默丢弃与 Signal Integrity 约束不符；工作语义、权限、卡片归属等架构边界仍保持 |

上述 FAIL 不表示所有实现无效。已确认的价值包括真实 Pi run boundary、原生会话观察的主体路径、精确归属的 UDS 连接、自动输入 HOLD、观察与 Claude 重入回执的类型隔离，以及明确拒绝 Pi 自动重入。

## 独立复核

事实和命令摘要在 [review-results.json](review-results.json)。负责人在隔离 HOME 下，从当前源码重新构建测试二进制，然后运行真实 npm Pi 和官方 standalone Pi 1.1.0，使用项目已有 deterministic provider，无模型账户。复验 scenarios 为 boundaries、disable、native_sessions，以及驱动强制包含的 settings_loop、start_both，共 71 项通过，exit 0；这不声称重跑了原报告全部 140 项。

前端完整 scripts/ui-tests 独立运行 771/771，exit 0。后端独立回归为 1209 单元测试与 44 个安全扫描/回归测试全部通过（exit 0），另有 13 项既有 ignored；包括 Claude 重入、procinfo、scheduler，以及 edr_quiet、external_admission、log_privacy、signal_census。执行者 WK 的 JSON、verdict、应用日志、smoke driver 和页面代码已核对；本轮负责人未再次启动 GUI，因此真实窗口结论仍属于“审查执行者证据”，不是负责人亲自重跑。未核实其所有历史私有日志和十二次远端环境状态，不能把报告自述转换为独立证明。

两个独立对抗实验直接使用本次重建的 production listener/transport 测试载体，没有修改 Deck 生产代码：

1. F01：隔离 pane 中的 C peer 获得 hello accepted 和 working ACK 1，保持 PID 与 socket，exec 成另一个根内 C 可执行文件，再发 turn-done。后端返回 ACK 2；生产 poll 同时返回 fg=exec-after、agent=turn-done、pi_signal=connected。scheduler 的 Pi HOLD 仍为 true。这是实际 exec，不是模拟 PID birth 改变，也不是“真实 Pi 被入侵”的宣称。
2. F06：一个连接每 0.5 秒发送空格但不完成首行；另一个 Pi 协议探针连续 3.53 秒收不到响应。关闭慢连接后探针立即取得 bad-frame 拒绝，证明在它之前没有完成分发。源码的两秒超时是单次 read 超时，不是整条首行的绝对期限。基线 1bdcf57 已有此缺陷，不归因于本次新引入；但当前实现不能因此声称满足 A24 和设计的共享 listener 有界性。

所有本轮 native/对抗实验均通过 experiment registry 清理，exit 0，无遗留 socket。最后再次核对五个实验根，registry show 均 exit 0，没有仍运行的登记资源，进程表没有根内可执行文件。原始私有记录与两个可复现 probe 脚本保留在 /private/tmp/deck-pi-accept-n_kowcmb。成功实验根分别是 /private/tmp/deck-pi-3sql7ynm、/private/tmp/deck-pi-hm_25dtd、/private/tmp/deck-pi-lufj0f2j。

F01 实验有两次设施失败，均不算产品测试结果：系统 Python 的进程登记被 registry 拒绝；复制系统 Perl 元数据失败（Operation not permitted）。没有绕过 registry 或修改系统权限。重新定位设施原因后，改用临时根内从 C 源码编译的两个可执行文件完成复现；失败实验也执行了 registry cleanup。未调用宽泛 kill、未操作已有 Deck/tmux 会话。

## 实质发现

### F01 旧连接跨 exec 仍然可信

严重程度：高。app/src-tauri/src/agent_pi/transport.rs:401 的 verified 只检查 pid/birth、tty/pgid、祖先关系；没有执行映像或 exec 实例连续性。已有 transport/tests.rs:580 的 later birth 是 PID reuse，不是同 PID exec。独立实验给出反例，违反 A12。原详细设计列出了 A12，却只具体规定 pid/birth 等检查，设计负责人也承担这一缺口，不能全部归因于执行者。修复要求见 Follow-up；运行实例连续性与 P3 供应方产品认证必须保持区分。

### F02 不支持的 Session ID 被静默丢弃并重置历史

严重程度：高。transport.rs:663 对 NativeId::Unsupported 只调用 forget_pi，然后返回普通 ACK。lifecycle.rs:871 的前端闭集只有 Observed；无法呈现 unsupported-native-id。observer.ts:149 对超过 512 字符的值直接返回 null，也没有显式降级。

更实质的问题：transport/tests.rs:1223 明确把“257 字节 start 被 ACK 且忘记，然后不同的 256 字节 snapshot 在同一 lease 接受”写成成功测试。相同机制可让原本已观察的 ID A，经 unsupported 事件清空记录后，接受 ID B；没有 teardown，却失去一致性检查。违反 B02 和设计原生 Session 连续性约束。不能只补一条日志或改测试名称。

### F03 关闭一个能力会终止另一个仍启用的能力

严重程度：高。transport.rs:303 发现任一 capability 不再启用就 end 整个 lease；end 同时清 Signal 和 lifecycle。observer 收到 capability-disabled 后永久 ended，直到 Pi 自己 reload。both→signal 因此丢失 working/turn-done 和未读 episode；both→lifecycle 同样丢失 Session 观察。mismatched 仅检查 live lease，结束后配置又可显示 current。

真实 WK 的 pi-session checkpoint 只断言 lifecycle 被撤销和 signal 配置仍为 true，没有断言 Signal 继续工作（app/ui/test/pi-live-smoke.mjs:224）。这是测试漏项，违反 P2 不破坏 P1 的阶段隔离要求。

### F04 必需的可见 UI 验收未完成

严重程度：中，验收门禁。G06 明确 UNRUN。C03 要求的真实 Review 截图也没有；pi-review checkpoint 调用 preview IPC 和 DOM-free model，并没有打开实际 Review 对话框。不能从模型结果推断渲染、长文案、缩放、混合卡片布局通过。补齐中英、1280×800、font scale、长标题、mixed/noCandidate/failure，不要求用户点击。

### F05 最终构建证据关联不充分

严重程度：中，证据门禁。native-result.json 的 facts.artifacts.deck_head 是 812b786，而报告称 final build；没有测试可执行文件哈希和当时 dirty diff 的记录。gates.json 使用“加入此文件的提交”描述 tree，WK 摘要也没有最终 carrier/build manifest。这不证明测试用了旧代码，但不足以证明最终代码与所有证据一一对应。负责人确认 observer 四文件 hash 与当前源码相同，并独立重建重跑 71 项；没有据此替执行者认证全部历史 140 项。

### F06 共享首行读取仍可阻塞所有协议

严重程度：高，可用性及门禁。agent_status.rs:1454 同步读取首行，timeout 每次 read 重新生效；整个 listener 串行调用 handle_stream。Pi 的 32 streams/128 queue/worker 上限在此之后才生效。独立滴流实验说明“所有读有界、恶意客户端不阻塞其他 Signal”尚未成立。修复必须限定为既有 listener 的有界首行准入，保留三类协议的归属与旧 helper/receipt 语义，不能引入通用插件服务。

## 已接受的偏差与保留风险

按 exact key sets 区分帧、批量复用“所有本批帧已读完之后”的 pane listing、心跳不发 tmux listing、保留用户 negative entries、source 字段命名、宪法文件不改，均可接受。批次之后到达的新帧进入新 listing 批次，未发现报告所述优化降低这项时序保证。

缺失 Pi agent 目录时保守拒绝，可作为本轮 UX 限制保留，不是身份事实；文案不能由“目录不存在”推断整台 Mac 从未运行 Pi。保持现有用户目录/配置完整。wrapper/任意 node 未识别的残余已由原设计公开，不能扩大成“所有 Pi 均有自动发送保护”。

未选入独立缺陷：hello 后提前夹带 frame 的非规范客户端会触发首行 remainder 丢失，但正式 observer 等 accepted 才发送，不能据此宣称正常 Pi 会随机丢帧。F06 修改首行分发时应明确处理已读 remainder 或明确拒绝，避免将原有单次消息读取直接当 stream framing。

系统通知/Dock 的真实发布、带账户的 Claude/Codex 混合运行、签名/公证发行包和其他 OS 矩阵仍未获得本轮独立实测。可以按对应范围 BLOCKED/UNRUN，不能覆盖为总体 PASS。无需读取用户账户或修改系统安全设置来补证。原报告中未运行 cargo audit、CE parity、tunnel-helper gate 不能写成“完整 CI 全绿”；修复应按实际改动与既有 CI 要求决定并如实列出。

最终建议：保留现有实现，执行范围收敛的中文 Follow-up 后重新验收；当前不建议作为 Pi 已验收完成的版本发布。
