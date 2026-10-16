# Phone Connector 修复与隔离验收（2026-10-16）

状态：修复完成，隔离功能验收 21/21 通过；完整 Rust workspace 仍有一项已保留的失败，详见下方。源码基线 `a326cdf3a6b3f1b18176da9079659e4762408eca`；本目录记录后续修复，不改变[原始诊断](../../phone-connector-diagnosis-2026-10-16.md)的历史结论。

## 已实现

- 列表共享范围与操作权限分开：默认保留 Codex/Claude 卡片范围；Mac 端明确确认后可共享全部卡片概要。手机保留主机、项目、列、卡片顺序和最近同步时间。普通 shell、空命令和 Pi 的概要可见不代表允许读取终端、发送、编辑便签或排队。
- 统一按能力和原因呈现输出、发送、便签及队列状态，修正停止会话和 MCP 控制卡片的误导状态；后端继续执行原有准入和身份检查。
- 前台周期更新列表及当前详情；合并重复读取，后台取消；响应须匹配配对、卡片、generation 和请求序号。网络中断时明确标记旧快照，身份或权限变化清除旧内容。
- 输出成为默认详情；刷新与跳到末尾各自独立，显示采集时间，支持换行。首次定位末尾，后续变化不抢滚动位置。便签为次级入口；大字号和横屏将输入区放入可滚动内容，避免遮住输出。
- 操作恢复独立于读取，有限查询原 ID，既不阻塞列表，也不自动重新 POST。已确认回执先协调本地草稿，列表/详情随后后台刷新。启动完成从当前生命周期状态恢复调度，避免旧 scenePhase 关闭自动确认。正常界面展示送达状态，原始 ID 与错误码位于诊断展开项。
- 新便签及编辑草稿持久化，离页/重启恢复。恢复后的草稿无需再次编辑即可提交。命令在首次提交前记录精确草稿 revision；只有同一 revision 的成功结果能清理草稿，相同文字或同秒时间不能代替身份。

这些改动遵循产品宪法的三条边界：减少反复查看和重建上下文；观察、权限、输入就绪分别判断；Deck 不推断 Agent 任务完成情况，不自动移动卡片。当前 Connector 没有可直接投影的可信注意力事实，因此没有伪造“需要关注”或 Agent 工作状态。

## 验证记录

最新 UI：5/5，通过；Agent 退出到 shell 的专项 UI：1/1，通过；均无失败和跳过。包含小屏深色、中文最大无障碍字号、横竖屏、真实 Codex 输出、停止会话、前后台、重启、普通 shell 概要权限、新建及编辑草稿恢复后直接保存。AppModel 15/15 通过、无跳过，包含真实 host/CAS/Keychain、乱序及旧 generation、慢读下保存确认、后台暂停与回前台恢复且不重发。两套源代码指纹一致，驱动 exit 0；见[完整验收报告](acceptance-report.json)。

保存问题的最后一段根因是启动/生命周期竞态：异步启动末尾使用旧 scenePhase，可能覆盖已经发生的 active 事件，关闭刷新及原操作恢复。诊断中 POST 约 0.4 秒返回 accepted、桌面约 1 秒后 applied，手机却长期 awaitingFinal。修复后同一重启保存路径约 2.4 秒自动确认并清空对应草稿，编辑草稿随后保存并读回成功。回执也已与远程读取解耦，慢输出不能阻塞已确认保存。阶段日志仅在 Debug Simulator 的显式诊断模式启用，限制 64 KiB，记录阶段、长度和状态，不记录文本、标识或凭据。

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| Simulator UI | 5 通过 + shell-return 1 通过 | [UI 报告](ui-report.json)、[日志](ui.log) |
| AppModel | 15 通过、0 失败、0 跳过 | [报告](appmodel-report.json)、[日志](appmodel.log) |
| Swift Core | 77 通过、1 opt-in 跳过、0 失败 | [日志](swift-core.log) |
| 前端 | 849 通过、0 失败、0 跳过 | [日志](frontend-final.log) |
| Rust Connector 模块 | 64 通过 | [workspace 日志](workspace-final-attempt.log) |
| Signal census | 9 通过 | [日志](signal-census.log) |
| Clippy / fmt / 原生构建 | 通过 | [Clippy](clippy-final.log)、[构建](host-build.log) |
| 清理规则 | 4 通过 | [日志](registry-final.log) |

完整 Rust workspace **没有一次最终全绿结果**：最近全跑主测试 1,254 通过、1 失败、13 忽略，exit 101。失败为 `agent_reentry::tests::a_controller_killed_at_any_phase_leaves_no_replay_and_no_agent_data`，预期 SIGABRT 的子进程退出 101，原因未定；该断言未修改，精确单跑 1/1 通过。后续目标分别通过：tmux 111、status helper 6、lifecycle helper 8、MCP runner 10、MCP stdio 5。不能把这些组合结果称为完整 workspace 全绿。

历史失败、排查中被后续证据修正的解释与环境异常见[验证历史](validation-history.md)；机器可读结果见 [results.json](results.json)。截图已目视检查：[实时输出](live-output.png)、[中文列表](chinese-card-list.png)、[中文竖屏](chinese-portrait.png)、[中文横屏](chinese-landscape.png)。

## 隔离与限制

桌面 host 使用一次性 debug bundle、独立数据目录、进程内 smoke 凭据与根目录内 tmux socket；进程/server 登记后仅通过 experiment registry 清理。iOS 使用新建的一次性 Simulator 和单次配对，通过真实 loopback pinned HTTPS、WK bridge、真实交互式 Codex 验证。未替换已安装应用，未修改生产配对/共享设置，未发布版本。

模拟器验收不等于真机验收：相机扫码及拒绝权限、Local Network 权限、Wi-Fi/VPN、Mac 睡眠、锁屏 Keychain 和后台回收仍需物理设备验证。测试未解决普通 shell 内手动启动 Agent 的远程准入；这仍需独立的显式目标绑定设计。

最终隔离审计：原有 11 个可用模拟器、0 个启动中；本轮实验无残留进程，测试 bundle/data 已移除，生产 Deck 原进程仍在运行。见[环境核对](final-environment.json)、[UI registry 审计](ui-registry-audit.log)及[AppModel registry 审计](appmodel-registry-audit.log)。

最终来源：HEAD `a326cdf3a6b3f1b18176da9079659e4762408eca`；源代码 diff SHA-256 `dd6ac0c75dc713712f170391fa8d2fe9466bb2219aa25314e87c5eeb166a115e`；原生二进制 SHA-256 `53d06100a8a82b8feb06d7deba24d320e04d5223788240d2abdc6b047fe6e23d`。Xcode 27.1 / iOS 27.0，iPhone SE（第 3 代），深色。未提交、部署或发布。
