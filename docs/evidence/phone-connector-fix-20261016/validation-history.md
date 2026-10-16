# 验证历史（保留失败及后续解释）

以下是过程中记录的阶段状态，最终验收以 [README](README.md) 为准。

## 验证记录

最新 AppModel 验收 13/13 通过、无跳过，包含重启后无需再次编辑即可保存草稿的回归；该隔离实例已完成 registry 清理。最新 UI 套件正在复验。Swift Core 总计 78 项：77 通过、1 个按设计跳过的 opt-in transport 测试；真实 host 已由 Simulator 套件覆盖。前端 849/849 通过。详见 `results.json`；XCTest 原始结果包的本机路径保存在运行报告中。

横屏视觉补测已通过窗口方向和截图尺寸断言，避免把旋转过渡帧当成最终布局。同轮 UI 中另有一项因 XCTest 无法计算非交互 StaticText 的激活点失败（`visual-followup-initial-ui.log`），已将文本可见性检查改为屏幕几何检查；按钮仍要求可点击。最终全套同时加入重启后直接保存便签及编辑草稿的回归。后续复验进一步发现大幅惯性滑动会越过短横屏中的整行，以及保存后对已滚出屏幕的编辑框直接查询 AX value。测试改用横屏短距离拖动，并把编辑框滚回屏幕后检查清空状态。该调整后中文横竖屏通过，但保存仍失败：录像确认界面停在“正在保存”，虽然桌面已记录 applied、手机 journal 已清除对应草稿。最初怀疑保存确认等待远程读取；解耦后慢读取回归通过，但真实 UI 仍失败，因此该解释并不充分。已将回执及本地草稿协调提前，远程刷新改为配对身份约束下的后台任务，并增加慢读取尚未结束时保存必须完成的回归；阶段记录进一步表明 POST 约 0.4 秒返回 accepted、按钮任务已结束，桌面约 1 秒后 applied，但手机 24 秒后仍 awaitingFinal。启动闭包在异步恢复后写入捕获的旧 scenePhase，会覆盖已到达的 active 事件，关闭自动确认。现改为生命周期事件唯一写入前台状态，绑定完成从模型当前状态恢复，且核验配对 epoch；正在重新验收。见 `lifecycle-diagnosis-*`、`save-confirmation-failure-*` 和 `save-confirmation-failure.png`。每个用例恢复竖屏，避免一次横屏失败影响后续用例。相关失败日志与无障碍树均保留。

首次小屏深色验收发现最大无障碍字号下主机信息/共享说明占满首屏，卡片未进入可见区域，中文测试失败。列表已将详细说明折叠，测试继续要求卡片可通过正常滚动到达；将保留该失败并用新构建重跑 UI。

首轮试跑：iPhone 17 UI 5 项及 Agent 退出测试 1 项通过。截图检查仍发现大字号输入区遮挡、首次输出位置不佳，已修改。AppModel 阶段检测到编译期间源码变化而中止，未将其记为通过；隔离资源已清理。见 `pilot-*.json`。

首次前端 gate 的两项既有 1 秒性能断言在编译并行负载下失败；保留 `frontend-initial.log`。未修改断言，编译结束后原命令完整重跑：849/849 通过，见 `frontend-final.log`。首次 Clippy 报参数数量过多，代码重构后完整 gate 通过；保留初次及最终日志。首次 workspace 主测试 1,255 通过、13 忽略；后续 Signal census 发现函数拆分后的旧归属项，已将归属改为实际辅助函数，NotSignal 分类及次数断言不变。第二次 `cargo test --workspace` 仍为 exit 101：主测试 1,254 通过、1 失败、13 忽略。失败项 `agent_reentry::tests::a_controller_killed_at_any_phase_leaves_no_replay_and_no_agent_data` 在 kill 阶段预期 SIGABRT，子进程却退出 101；子进程 panic 细节随原测试临时目录被清理，原因未定，不将其归因为负载。该测试空闲时精确复核 1/1 通过，未修改断言。后续目标单独完成：tmux 111、status helper 6、lifecycle helper 8、MCP runner 10、MCP stdio 5；Signal census 9/9，fmt 和完整 Clippy 通过。该组合证据不等同于一次完整 workspace 全绿。

另有一轮在 UI 开始前因 `ps` 全参数枚举超过 10 秒中止。Registry 拒绝清理未登记资源；核对本轮唯一根目录内的 host、tmux server 和附属 client 后补齐登记，随后 registry 清理与审计通过。脚本改为只读取 PID 和可执行路径，不读取完整命令参数；没有绕过清理拒绝或停止其他进程。见 `environment-timeout-report.json` 及 `environment-cleanup-*.log`。


## 最终复核

`receipt-appmodel-failure-*` 保留一次真实 host 命令在截止时仍 executing 的失败；13 个其他模型用例通过。未放宽断言，最终相同真实 host 测试及全部模型用例 15/15 通过。保存 UI 的只读阶段日志见 `lifecycle-diagnosis-stages.log`；它纠正了仅凭录像将旧显示解释为 submit 一直等待的结论。最后采用生命周期事件单一来源及当前状态恢复，原 UI 路径完整通过；见 `final-ui-stages.log`、`ui.log`。全部历史失败未从证据中删除。
