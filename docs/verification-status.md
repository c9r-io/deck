# 当前验证索引

更新：2026-10-13。这里汇总近期改进的验证范围与下一次验证触发条件，
不是对所有功能的重新认证，也不替代各模块头部契约。

状态：**PASS** 只对所列代码和环境成立；**沿用** 表示没有在本轮重跑原生场景；
**待观测** 表示现有结果不足以证明间歇问题消失；**BLOCKED / UNRUN** 保留原记录的含义。
一次 CI 通过不会把原生验证、发布安装或上游能力缺口自动变成 PASS。

## 本轮 Board 与授权边界治理

基线 `e1bd78e`；本轮本地改动与命令结果见
[Board / authority 治理证据](evidence/board-authority-governance-20261013/README.md)。
这是本地验证，不是该工作树的远端 CI 或原生重新认证。

| 项目 | 本轮范围 | 原生范围与触发条件 |
| --- | --- | --- |
| 冻结 channel head | documents 的最小类型投影供实际 admission 消费；明确 false 与缺省值分开；独立字段篡改、扩展值、恢复/重放继续回归 | 不改变传输、readiness、grant 语义或锁顺序，沿用下表原范围；未来改变任一语义时重跑对应原生场景 |
| 撤销时点 | tick candidate 的事务回滚/一次撤销；隔离真实文件与生产 send pipeline 在 prepare/final_probe 后撤销，确认零 fire 与持久撤销 | fake probe 不是真实 pane 输入认证；pre-start/pre-fire 栅栏均保留 |
| Board 卡片展示 | `board-card.js` 负责 DOM 与手势路由；Board 保留动作 owner；完整 UI gate 与新增 DOM 行为测试 | DOM/CSS 与原主体一致；本轮不重跑 WebKit 尺寸/focus/click smoke，未扩大旧 PASS |

本轮最终门禁、构建来源、退出码和文件哈希统一记录在上述证据目录，历史结果不转写成新代码的 PASS。
本地 UI 786 项、Rust workspace 1,520 项通过（原有 13 ignored）；fmt、clippy、模块检查与
release 二进制/来源检查通过。远端 CI、Rust llvm-cov、真实 GUI/客户端和签名安装本轮未执行。

## 上一轮 hooks 与验证同步

基线 `84b7783`（0.8.1）；该轮结果已在 `e1bd78e` 提交，文件哈希、命令、结果和失败经过见
[治理证据](evidence/technical-debt-20261013/README.md)。
基线的 [test](https://github.com/c9r-io/deck/actions/runs/38284431702) 与
[nightly](https://github.com/c9r-io/deck/actions/runs/38284433607) 已通过，
它们不覆盖之后的 `e1bd78e` 治理修改或当前工作树；本轮未核验更新的远端 CI。

| 项目 | 已有证据和范围 | 仍未证明 | 下一次验证触发条件 |
| --- | --- | --- | --- |
| tmux 测试启动与重建同步 | 本轮检查每条预期成功命令的退出码；重建前观察旧 server 的内核退出事件；完整合约测试与重复拓扑测试，见本轮证据 | 历史 CI 的 shell prompt 超时未在本机复现，仍为**待观测**；不声称彻底消除 flaky test | 下一次正常 push 的 CI；若再失败，保留本轮新增的具体命令、退出码与 stderr，按失败阶段定位，不靠重跑变绿 |
| Pi transport 测试同步 | 心跳场景通过 fixture 的写入完成标记同步；保留普通 observer 的 nonce 重试行为；完整 transport 测试，见本轮证据 | 真实 Pi 的 native 矩阵没有在本轮重跑；定时失效测试仍有其独立时间预算 | 改 transport、observer 或 fixture 协议时重跑相关单测；改变进程证明语义时重跑 native 矩阵 |
| hooks 配置模块 | 配置、安装、迁移与对应测试搬入 `agent_status/hooks.rs`；生产主体逐段比对仅空白变化；原 IPC 路径通过父模块导出 | 没有在本轮启用用户的真实 hooks，也没有重新执行签名 release 的设置开关 | 修改编辑器、路径准入、迁移语义时重跑配置测试及相应真实窗口场景；仅移动代码不能扩大原生认证范围 |
| smoke 剪贴板读取 | 固定 `/usr/bin/pbpaste` 与 `LC_ALL=en_US.UTF-8`，避免缺失 locale 或继承 C locale；命令构造、smoke 禁用保护与 EDR 检查 | 本轮没有读取或改写用户剪贴板，也没有重跑真实窗口的非 ASCII 拷贝场景 | 下次 clipboard WK smoke 在缺失 locale / C locale 两种启动环境下检查字节；失败时区分生产 writer 与 smoke reader |

## 近期功能的原生验证范围

| 能力 | 当前可沿用的证据 | 不应写成已完成的部分 | 重验触发条件 |
| --- | --- | --- | --- |
| Pi run 边界与 native session 观察 | [Pi 收尾](evidence/pi-closeout-20261012/README.md)：P1a、P2 的 native 矩阵与窗口证据绑定 `8bb2b7f`；后续产品改动的适用范围在原文逐项说明 | needs-input、re-entry 为 **BLOCKED**；通知实际弹出、其它 Pi 版本、签名发布包等仍按原表 **UNRUN**。已接受残余风险不等于消除 | Pi 原生协议／运行时改变，或观察者／身份绑定逻辑变化；上游提供可靠输入信号或代码身份证明时再评估缺口 |
| Codex shared-daemon Signal | [归属认证](codex-signal-certification-20261002.md)：共享 daemon 的身份归属限制；embedded topology 与 shared daemon 分开判断 | 无可验证的每客户端 pane 绑定时仍不可授信。安全拒绝不等于注意力覆盖完整 | 官方来源提供新 provenance 原语，或改变 hook 进程拓扑时重新认证；不以 cwd、时间或会话 id 补造归属 |
| Claude 重启后 re-entry | [验收矩阵](evidence/closeout-20261011/reentry-acceptance.md)与[收尾状态](evidence/closeout-20261011/README.md#verdict)：debug，以及迁移开启观察的 Developer-ID release carrier，在 relocated home 的代表性事务 | `/Applications/deck.app`、release 设置开关、真实窗口中的未知结果、macOS 26 真实客户端等仍受原表限制；私有载体未公证 | 改身份验证、启动／退出 receipt、事务或安装位置时重验；需要实际安装验证时使用满足隔离条件的环境 |
| Slack 与频道首步 | [首步本地验收](channel-first-send-verification.md)、[后续收尾](evidence/closeout-20261011/README.md#b-the-slack-integration-in-the-real-window)：合成事件／隔离 fixture 与后续真实 Slack debug 窗口证据分别成立 | release 窗口、channel-scope 故障和跨退出 pending 事件等不因通用 gate 通过而关闭 | 改 grant、冻结计划、撤销、重放或最终发送时重验相应单元／故障用例；改传输时补真实服务验证 |
| 构建产物来源与实验清理 | [收尾 A 与 registry](evidence/closeout-20261011/README.md)：已修复共享产物污染、打包后复核与资源登记；本轮继续执行相应回归 | 历史误杀的会话不可恢复；之后的通过不抵消事故。旧 runner 的未重跑记录仍保留 | 新增打包入口或实验 runner 时，先接入 provenance／registry，再运行；不得按默认 socket 名清理 |

## 治理停止边界

上一轮只拆 hooks 配置；本轮只收窄 channel head 证据表达和一个 Board 卡片展示职责。
Board 的 polling/lifecycle、其它来源的 JSON 授权消费、settings 大模块和构建系统仍不在本轮范围。
唯一 writer、副作用入口、恢复来源、撤销条件与锁顺序保持原状；以职责和依赖边界为验收依据，
不以文件行数作为完成条件。第一方静态表继续保持窄接口，不扩成通用插件系统。

更新此页时保留被测提交和环境；新代码若改变旧结论的前提，先把对应行标为待重验，
不要仅更新日期。证据中涉及的凭据、用户文件内容、内部地址和 native session id 不收录到这里。
