# Pi Follow-up 01 独立验收（2026-10-12）

**结论：本轮不验收通过。F02–F06 在约定范围内通过，F01 仍 FAIL。**

审查基线：代码提交 `eb16c7123f001149ec3d3aa6c4e8f5b78bc45d1a`，当前证据提交 `546eb45d648589875c5bfee7d07ce10d6225aa4e`。审查开始时工作区干净；负责人没有修改生产代码或仓库测试，只新增本验收材料和 [Follow-up 02](../../design/pi-v0.8.0/follow-up-02.md)。

独立计算的 324 个源文件摘要为 `6388ddd457ae330231a6506df19119a0e30b98ad2026393f9b73091402511549`，与执行证据相符。`CLAUDE.md`、`AGENTS.md` 未被修改。已审查修复差异、模块契约、协议/observer、lifecycle、状态投影、测试设施、门禁记录和截图；另有独立只读安全审查复核 F01。本文件不是执行者自评的转述。

## 判定

| 项目 | 独立结论 | 依据及范围 |
| --- | --- | --- |
| Design | FAIL | 登记后的 exec 连续性原语可接受，但建联边界与每帧发送者证明尚不完整 |
| Implementation | FAIL | R01 已在生产后端复现，R02 尚未关闭 |
| P1a Signal | FAIL | 正常 working/turn-done 通过；后台子进程可代写状态 |
| 完整 P1 | BLOCKED | Pi UI 等待原语的既有上游限制没有解除 |
| P2 Lifecycle | FAIL | 正常两种 runtime 的会话切换通过；同一子进程还能代写 Session snapshot |
| P3 Re-entry | BLOCKED | Pi 运行代码供应方身份前提未满足；`PI.reentry=None` 的拒绝路径保持 |
| Security | FAIL | 当前 frame 的实际写者不一定是被验证的前台进程 |
| Regression | PASS（已测范围） | 独立 backend/frontend/四组安全检查通过；测试驱动另有 R03 问题，未声称账户及发行环境全部通过 |
| Native E2E | FAIL（完整验收） | 既有定向场景 170/170 通过，新增 adversarial fixture 违反准入契约 |
| WK / Layout | PASS（F04 范围） | 审阅真实窗口证据与截图；本轮没有重新开窗；通知/账户流程未获验证 |
| Product Constitution | FAIL（Signal Integrity） | Agent 工作所有权、无自动授权、无编排扩张仍符合边界；可靠归属这一硬要求未满足 |

P1a/P2 的失败来自它们共同的传输归属缺陷，不是由 P3 阻塞传播，也不否定已经通过的会话 reducer、独立撤销等成果。

## 发现

### R01 高：没有终端的子进程可以通过继承连接写 Signal 与 Session

**已独立实测。** 复现用根内编译 C fixture，运行于真实隔离 tmux pane；Deck 使用从当前代码构建的测试可执行文件，经过 production listener、transport、store、poll。此反例是 adversarial peer，不冒充真实 Pi 的正常运行测试。

1. 前台父进程完成 hello，working 得到 ACK 1。
2. 它 fork；子进程调用 `setsid()`，父进程仍在原 pane 前台且没有 exec。
3. 子进程的 PID=PGID=SID，与父不同，`ps` 的 tty 为 `??`。
4. 子进程通过继承 FD 发 `turn-done`，得到 ACK 2；再发 lifecycle snapshot，得到 ACK 3。
5. 同一 poll 显示 `agent=turn-done`、`pi_signal=connected`、`pi_session=observed`，且产生未读 episode。

证据：[Signal 单独反例](inherited-fd-signal.json)、[Signal + Lifecycle 反例](inherited-fd-both.json)、[当时运行的探针](inherited-fd-probe.py)。后者保留当时本机路径，复用时需改为当前 checkout/test binary/已校验 runtime 路径；它只在新临时根里工作。正式 Session ID 不在这些文件中，probe 使用显式的测试字符串。

**根因。** `app/src-tauri/src/agent_pi/transport.rs:938` 的 `Service::hand` 保存父 peer/watch；约 745 行的 `committed` 及 529 行的 `verified` 仍读取父 generation。父亲的进程事实和 watch 均正常，不等于这段 frame 由父亲写出。

**违反契约。** 设计 README 的 D7、传输模块 “Who may bind” 都排除后台 child/helper。执行报告把 inherited-FD child 称为“原设计已公开的同 pane 残余”不准确。同一 Pi 进程内 Extension 自报错误，与另一进程代写不同。这里没有证明获得任何执行权限，但注意力与 Session 观察已经被错误归属，足以使 P1a/P2 FAIL。

### R02 高：watch 不能追溯其登记前 hello 的执行实例

**源码事实。** `agent_status.rs:2053` 在首行读完、排队后调用 `Service::hand`；后者才取得 peer 并登记 watch。`transport.rs:1163` 附近会提交随 hello 已读到的 `rest`。执行报告 §3 F01 残余第 1 条明确承认该 exec 窗口。

**推断及原语边界。** 旧实例若先写 hello/帧，再于登记之前同 PID exec，新的 watch 只能观察之后发生的 exec，无法为已读或内核中已缓冲的旧字节作归属证明。影响不止 hello 的版本/能力声明：实现接受 remainder 中的状态。新映像“也可以自己 hello”不能让旧实例的状态变成它实际发出的状态。

此精确时序本轮没有独立 native 复现，不伪装为实测；缺口由代码和已承认的 watch 语义确定。原 Follow-up F01 明确要求 hello 绑定也不得跨实例沿用，不能由执行者另批残余。仅将 watch 前移到读取之前，仍不能排除 accept/watch 前内核已缓冲旧字节，后续修复须给完整建联边界证明。

登记后的 same-PID exec、静默 exec、磁盘文件替换的现有实现和定向 native 测试通过；不要求废弃这部分。多 watch 登记时是否会漏掉极窄窗口里的事件，目前没有足够证据判为第三个产品缺陷，仅要求修复时核查。

### R03 中：native 驱动的 stdout framing 可能丢首条事件

直接执行原驱动时，44 项通过后，第二个后端 ready 超时，进程 exit 1。私有日志中后端仍正常接收 Pi 的 snapshot/heartbeat，registry 最终清理 exit 0。不能把这一运行记为 PASS。

`scripts/pi_native.py` 的 `Tap.read` 仅认 `line.startswith('@tap ')`。独立重跑保留的 raw stdout 确认 libtest 会输出如下同一行：

```text
test agent_pi::tests::native::tap ... @tap {"now": ...}
```

原解析器会丢掉这一条。首条由 sampling 与 ready 竞争决定。**首次失败未保存 raw stdout，因此当次 ready 是否正好带此前缀属于有根据的根因判断，不是已截获的事实。**

负责人仅在临时驱动副本中识别这个确切 libtest 前缀并保存完整 stdout；产品和测试可执行文件均未改变。随后定向矩阵 170/170 通过。见 [临时 reader](native-reader-probe.py)、[该次结果](native-focused.json)。执行 Agent 仍应修复正式设施并补 framing 回归，不能靠增加超时掩盖。

## F01–F06 逐项复核

| 项 | 判定 | 复核结果 |
| --- | --- | --- |
| F01 | FAIL | 登记后的 exec 修复有效；R01/R02 未被原契约豁免 |
| F02 | PASS | unsupported-native-id 显式投影、同 lease 粘性、Signal 独立保留；256/257/512/513 两种真实 runtime 复验 |
| F03 | PASS | 单向收紧、capability 通知先于 accepted、在途 revoked 不写状态、剩余能力继续工作；两种 runtime idle/active 与切换复验 |
| F04 | PASS（限定范围） | 真实 Pi UI 与 fixture layout 分列；Settings/Why/Review/Attention 可见证据、中英文/字号与最小窗口记录齐全；账户和系统发布仍 BLOCKED |
| F05 | PASS | 源摘要独立匹配；30 步 gate 均记录 exit 0；三份 WK 构建身份一致；保留的原 native carrier SHA256 独立匹配 `1b47b562…141fd1`。并非声称独立重跑了全部 30 步 |
| F06 | PASS | 首行绝对期限、32 槽位、32 队列、固定 dispatcher 与 remainder 处理合乎批准范围；真实 socket 的滴流/静默/partial、容量及跨协议测试独立通过 |

F04 抽看六张已提交截图（Settings 中文 160%、混合 Review 中英文、真实 Pi unsupported Why、真实 Pi Review、Board），并核查 checkpoint、operator 检查与布局脚本。窗口内容中的混合 Claude/Codex 明确为 fixture；现有窄卡片 attention 徽标挤掉标题的问题已有单独记录，未伪装为布局零缺陷。没有证据需要为本 Pi 修复扩大到重写卡片布局。

## 设计裁决

同意 kqueue 作为**登记后的**连续性事实，不同意它被当成过去字节或实际写者的证明。exec 后撤销旧证明、不可问时不声称 connected 的方向正确。Pi 的供应方身份/P3 门槛不变。

同意闭合 unavailable/capabilities/revoked、收紧通知先于 accepted、hello 能力交集、当前 lease 不扩权、重新启用等待 reload；同意 needs-reload 语义与 unsupported 文案。Pi 集成尚未发布的前提下，本轮 wire v1 调整可接受；后续握手变更仍须声明旧 runtime 的失效/重载语义。

同意有界固定 dispatcher 和先读完先分派的 hook 顺序；保留连接存活期间的内核取证、receipt deadline/结算顺序。满载拒绝的有限可用性残余已获 F06 范围批准，不能宣称无限负载下无丢失。这不构成 R01/R02 的豁免。

## 独立执行证据

| 层 | 本轮结果 | 边界 |
| --- | --- | --- |
| Rust backend | 1228 passed / 13 ignored，exit 0，570.64 秒 | 当前代码的 deck-app suite；不是全 workspace llvm-cov 重跑 |
| Safety / Regression | edr_quiet 20、external_admission 8、log_privacy 7、signal_census 9，均 exit 0 | 共 44 项 |
| Frontend / observer | `scripts/ui-tests` 777/777，exit 0 | 当前树 |
| Native 初次 | 44 passed，ready timeout，exit 1 | 不计 PASS；清理 0 |
| Native reader 修正副本 | 170/170，exit 0 | 真 Pi npm/Node + standalone/Bun；native_sessions、capabilities、on_the_way、unsupported_ids、exec_peer 及 setup |
| Adversarial C peer | Signal 与 lifecycle 均被错误接受 | 新增安全反例为 FAIL，不与正常 Pi 170 项相抵消 |
| WK / Layout | 复核执行者证据 | 本轮没有重开远端 GUI；未自行发布通知、登录 Claude/Codex 或测试 macOS 14 |

独立构建的测试可执行文件 SHA256 为 `82124a852b096a6eab633278d9bc1fe609abbc905392a22e2508597bb925f199`，正常 native 和 adversarial carrier 都核对此值。与执行者先前 carrier 的二进制 hash 不同，源摘要相同；没有混称同一制品。

准确命令、私有日志位置、判定与 cleanup 在 [review-results.json](review-results.json)。本轮没跑新的 fmt/clippy/coverage/release/signing 全流程，因为负责人没改生产代码；执行者的相应证据仅作为已审阅资料，不写成负责人自行运行。

**负责人自己的过程错误也保留。** 扩展 C probe 到 lifecycle 时，两次先后误写字段 `id`、`nativeId`，被 production parser 正确拒绝 `bad-frame`；随后按 `agent_pi.rs` 的实际协议 `nativeSessionId` 修正，得到本报告的 ACK 3 反例。两次错误测试均完成 registry 清理，不用于认定产品缺陷或产品通过。

所有本轮 native/probe 根在独立 HOME/data/session/tmux socket 下运行；每次 cleanup exit 0，最终再查 registry，登记项全为 gone，无活动 socket 残留。保留最后的 native 与双能力反例两个根；另四个已结束的本轮根再次核实后移入各自唯一命名的废纸篓目录，原始失败日志仍可审计，映射见 review-results。未动用户数据、旧实验根或既有废纸篓内容。本轮没有远端操作。Rust/前端日志保留在 review-results 指定位置。

## 剩余风险、阻塞与建议

待修复：R01 实际发送者绑定、R02 建联前旧字节归属、R03 测试输出 framing。前两项修好之前，不接受 P1a/P2 和整体 Security PASS。若现有安全边界内无可用原语，应以最小反例将共享前提判为 BLOCKED，不准降低标准。

继续保留：完整 P1 的可靠 needs-input 上游限制、P3 供应方运行身份限制；真实 Claude/Codex 账户、系统通知/Dock 发布、其他 Pi/macOS/Intel 和 macOS 14 候选包启动、签名/公证发行载体仍未全部验证。

建议独立执行 Agent 按 [Follow-up 02](../../design/pi-v0.8.0/follow-up-02.md) 完成收敛修复与自动验证，**全程中文报告**。不要求用户代跑测试，不推送或发布；保留 F02–F06 成果，下一轮重点重验 R01/R02 及其关联回归。
