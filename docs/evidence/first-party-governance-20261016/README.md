# 第一方插件职责治理与隔离验收

日期：2026-10-16（Asia/Tokyo）；基线 `3261805`。本轮在工作树完成，未发布或替换正式安装。

## 实现范围

延续自动注册目录，将 Slack 和 Bergamot 的状态、事件绑定与异步操作收归各自 Settings controller；Phone Connector、MCP（含 Tunnel 配置和授权对话框）与语音偏好纳入同一 contribution 机制。Settings 宿主统一挂载、刷新、语言更新、关闭与反馈，并继续持有唯一设置事务。原有导出委托给同一个 controller，不建立第二份状态或写入器。

后端新增独立 `access` 类型，按原顺序启动 Connector 和 MCP；语音偏好使用无 Rust 声明的 `settings` 类型。生成器拒绝伪造或 null Rust 声明、重复目录项及漏登记模块。MCP 授权、输入/重启守卫、Connector 配对、Slack 凭据和自动化授权仍由原生模块持有；注册和打开设置不会启用服务、授权客户端、录音或下载模型。

编辑器描述和自动化触发器继续留在原有核心边界，本轮不扩展其抽象。未改变持久化字段、schema、IPC 权限或用户操作流程。

## 用户体验与隔离

独立比较基线 HTML 和实际挂载结果：原有 400 个元素 ID 及属性完整保留，无缺失或重复；新增的 3 个 ID 仅为隐藏挂载位置。设置分类、条目顺序、搜索标签和关键词与基线相同。

所有原生实例使用临时根目录中的独立 carrier、HOME、数据和 tmux socket。进程与 server 启动即登记，停止只经 `scripts/edr_runtime.py experiment`。正式 `/Applications/deck.app` 未替换，生产配置未用于测试。

完整原始日志：`/private/tmp/deck-governance-20261016/logs`。机器可读汇总见 [results.json](results.json)，原生检查点见 [native-checkpoints.log](native-checkpoints.log)。

## 已完成的原生与前端验证

| 验证 | 结果 |
|---|---|
| UI 全量与覆盖率 | 847 通过；行 94.91%、分支 90.14%、函数 88.38% |
| Rust workspace 与覆盖率 | 1,527 通过、0 失败、13 原有 ignore；行 85.65%、函数 83.72% |
| Python 工具 | 190 通过，含目录生成器 9 项 |
| fmt / clippy、目录新鲜度、workflow、tmux/Bergamot pin、原生 bridge/剪贴板 guard | 通过 |
| App / Tunnel audit、Tunnel fmt/clippy/tests | 通过；App audit 保留 8 条既有 informational warning |
| CE 本地完整校准及覆盖率 sidecar 来源检查 | 通过 |
| release 编译、翻译链接、EDR 扫描、本地 ad-hoc 兼容包 | 通过；未安装、启动或发布兼容包 |
| JS 语法 / 依赖 | 178 个模块语法通过；73 个生产模块标识符与依赖检查通过 |
| 真实 WKWebView 综合交互 | 94/94 |
| Settings / voice | 12/12、18/18；Settings 包含 5 项新增 MCP 检查 |
| Connector / channel / approval | 5/5、3/3、12/12 |
| empty-start / signal-finish | 5/5、8/8 |
| Claude Hooks | 8/8 |
| Slack 真实连接与故障恢复 | 4/4，无消息发送 |
| 翻译 controlled / native-install / restart-enabled / restart-disabled | 20/20、12/12、4/4、11/11 |
| Pi npm / Pi layout | 各 16/16；layout 留存 64 张完整窗口快照 |

Slack 在用户指定的 Sandbox A 既有 App 上验证 User/App/Bot 凭据，仅经 Brave 与隔离 Settings 传入测试进程内存，未写入生产 Keychain。测试只监听不会匹配真实对象的合成规则，未新增权限，结束后销毁控制会话凭据变量。首次清理的即时进程列表观察到退出中的应用，registry 再次核验及独立进程/socket 检查均无残留；初次结果和复核分别保留。

翻译执行真实 HTTPS 下载与 Bergamot 推理，并目视检查译文界面。Pi 使用已有 npm 运行时、本地测试模型和合成配置，不读取用户模型凭据。原生结束后的正式 Deck 进程 PID/启动时间/路径，以及 settings.json、deck.json 的哈希和修改时间与测试前相同。

## 发现并修复的测试问题

- 控制器生命周期隔离测试首版尝试修改冻结的 controller，测试失败；改为在可变 controller 上注入失败，仍断言后续贡献均执行、所有结果收集完整。
- Rust 合约发现新增的 5 个 MCP 原生检查点未进入日志封闭词汇；补齐准确名称，没有放开日志限制。
- 新 Settings 原生日志有 12 个检查点、153 个按钮；旧回放夹具及负向变异仍固定旧检查点和 128 按钮。换成实际原生验收日志，并按当前数字生成同样的负向变异；缺失、失败与负计数断言保持。
- 代码复核发现生成器可接受后端 `rust: null`；修复键存在时的类型检查，补入负向回归。
- clippy 拒绝未使用的 access 唯一身份查询；启动入口现在实际使用相同的唯一身份规则，重复身份仍不启动。

## 最终复核

全部实验资源按登记表复核，应用、辅助进程和 tmux socket 无残留。正式 Deck 的进程 PID、启动时间与可执行文件路径保持；`~/.deck/settings.json` 与 `deck.json` 的 SHA256 和修改时间保持。

Rust 首轮完整命令在主应用阶段返回 101（1,251 通过、1 失败、13 ignore），没有将尚未执行的后续工作区测试计为通过；修复封闭检查点清单后，完整 workspace 覆盖率重新执行，最终 1,527 通过。release 编译保留未改动的 `smoke_native.rs` 中 `DELAY_NEXT_SETTLE_MS` dead-code warning，命令退出 0；clippy 的标准全 features 门禁退出 0。

## 验证边界

真实 Slack 验收只覆盖既有 Sandbox A 凭据、连接和故障恢复，不发送消息。语音原生检查不申请真实麦克风权限。macOS 14、远端 CI、真实 Slack 消息投递、生产安装及升级未执行，不能从本轮本机通过结果推定通过。

上一轮 Hooks 额外整文件往返检查发现的既有空 `Notification` 数组清理语义保留在[上一轮记录](../first-party-autoreg-20261016/README.md)，本轮未改变合并引擎，也不将该额外断言计为本轮通过。
