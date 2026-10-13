# 当前验证索引

更新：2026-10-14。这里汇总近期改进的验证范围与下一次验证触发条件，
不是对所有功能的重新认证，也不替代各模块头部契约。

状态：**PASS** 只对所列代码和环境成立；**沿用** 表示没有在本轮重跑原生场景；
**待观测** 表示现有结果不足以证明间歇问题消失；**BLOCKED / UNRUN** 保留原记录的含义。
一次 CI 通过不会把原生验证、发布安装或上游能力缺口自动变成 PASS。

## 本轮键盘导航与注意力返回治理

基线 `0743224f`；实现与证据见
[键盘导航与注意力返回治理](evidence/navigation-governance-20261014/README.md)。
卡片、侧栏和项目提供原生键盘打开入口，重绘保留焦点；从注意力定位后可直接返回，
继续打开同一卡片仍保留筛选、滚动与原行。大字号下标题与徽标分别占行，避免入口被挤至零宽。
定位不启动会话，普通用户打开动作继续使用原有语义；没有新增 schema 或授权来源。

完整 UI gate 816 项、五组原生合约 55 项、语法/模块及 fmt/clippy 通过。
完整 Rust workspace 1520 项通过、13 项保留原有忽略，0 失败。
隔离真实 WKWebView 51/51 检查点通过，其中新增导航检查包含 18 个条件；
中英文 × 100%/160% 的 4 组入口布局及既有 12 组注意力布局通过。
真实键盘验证项目 Enter/Space 切换和侧栏 Enter 打开。三次 smoke 按精确 socket 清理，
最终 inventory 退出 0。正式安装、真实 Agent 集成与远端 CI 未执行；投递恢复和注意力密度仍待后续治理。

## 上一轮 Settings 状态反馈治理

基线 `9a3b855`；本轮实现与证据见
[Settings 状态反馈治理](evidence/settings-feedback-governance-20261014/README.md)。
Hooks 读取失败、超时和缺失/非法字段显示未知并禁用，读取重试不写配置；
普通设置与 Hooks 保存/应用结果逐项显示，失败跨设置窗口关闭保留，侧栏提供数量与定位入口。
反馈仅在当前应用进程内保留，未增加 schema 或授权来源。

完整 UI gate 811 项、五组原生合约 55 项通过，语法/模块检查与 debug 构建来源复核通过。
真实隔离 WKWebView 注入 native 保存失败，验证回滚、跨重开提示和定位；
中英文、100%/160% 字号、8 个设置区共 32 组布局及 7/7 smoke 检查点通过。
测试资源已按精确 socket 清理，最终 inventory 退出 0。真实 Hooks 写入、正式安装、完整 Rust
workspace、fmt/clippy 与远端 CI 在该轮未执行；键盘入口的后续证据见本轮记录。

## 上一轮 Settings 提交一致性治理

基线 `ce872c1`；本轮工作树的修复、回归和本地验证见
[Settings 提交一致性证据](evidence/settings-write-governance-20261014/README.md)。
每次写入只应用该操作的字段修改；失败后从已落盘状态和剩余预览重建界面。
编辑器、语言和语音经过同一提交路径；原生语言更新在落盘后执行，生效失败不回滚已保存选择。

新增 18 项 DOM 回归；完整 UI gate 804 项通过，原生 IPC/EDR/隐私/Signal/所有权合约
55 项通过；debug 构建及 sidecar/frontend staging 来源检查通过。首次保存、规则撤销、
原生 channel grant 保留的既有测试继续通过。实际保存和原生菜单调用在新用例中均由 mock 驱动；
本轮未执行真实 WKWebView、用户配置、Slack/Agent 客户端、签名安装、远端 CI 或完整 Rust workspace。
以上范围不能扩大上一轮原生认证；修改 IPC、schema 或授权语义时须重验相应原生场景。

## 上一轮 Board 与授权边界治理

基线 `e1bd78e`；该轮已提交为 `ce872c1`，改动与命令结果见
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

前两轮分别拆 hooks 配置、收窄 channel head 证据表达和 Board 卡片展示职责。
此前两轮治理 Settings 写入、预览、失败回滚及状态反馈；本轮治理键盘入口和注意力返回连续性。
Board 的 polling/lifecycle、其它来源的 JSON 授权消费、名称字符/字节兼容性和构建系统不在本轮范围。
唯一 writer、副作用入口、恢复来源、撤销条件与锁顺序保持原状；以职责和依赖边界为验收依据，
不以文件行数作为完成条件。第一方静态表继续保持窄接口，不扩成通用插件系统。

更新此页时保留被测提交和环境；新代码若改变旧结论的前提，先把对应行标为待重验，
不要仅更新日期。证据中涉及的凭据、用户文件内容、内部地址和 native session id 不收录到这里。
