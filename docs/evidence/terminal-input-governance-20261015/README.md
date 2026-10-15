# 终端输入附着边界治理

日期：2026-10-15（Asia/Tokyo）；基线 `817df9b`；验证对象为该基线上的本轮修复。

## 发现与修复

安全复核确认一个既有 P2 缺口：文件拖入/图片粘贴、命令补全、ghost 补全和菜单清屏直接以会话名调用 `pty_write`，未携带附着代次。尤其文件读取与保存经过异步等待，如果期间同一会话重新附着，旧操作可能写入新附着。这不是已证实的跨任意卡片或远程越权。

现在这些入口与键盘共享每个窗格唯一的输入队列，在接受操作时捕获窗格和附着代次；首次附着尚未返回时绑定该次请求。文件读取/保存开始前即占用队列位置，后续输入不能越过。窗格或附着已替换则取消，既不改投也不重放；只切换分屏焦点仍写入原窗格，文件完成时不会抢回焦点。准备失败或无内容释放队列。

取消的是终端写入；已经开始的文件保存仍可能完成，产物保留原有私有权限和过期回收规则。

后端在选取 PTY writer 的同一把锁下要求代次匹配，缺省、null 或过期代次均拒绝。保留已有 MCP 控制栅栏和重启活动栅栏。生产前端只剩 `wireTerminalInput` 一个 `pty_write` 调用点，外部输入 census 随实际调用点更新；原有 smoke 的直接测试写入也明确携带代次。

## 验证范围

- Node 使用生产 layout、selection、completion 代码和 vendored xterm，控制 IPC 完成时刻：补全/ghost/菜单清屏在选择清理期间保持原代次；文件等待期间重附着/替换窗格、切换分屏、后续键入和首次附着等待；准备失败/空内容不阻塞，程序化输入不能冒充自动回复绕过语音输入互斥。
- Rust 使用真实 kernel PTY 和 Tauri mock IPC 参数解码：缺省/null/旧代次不写入任何字节，正确代次按顺序到达。测试读取后续有效哨兵行，避免把“调用报错”单独当作零写入证据。
- 原生验证使用隔离 debug bundle、临时数据目录和唯一 smoke socket。终端运行关闭回显的 `cat`，仅回传固定测试文本。页面控制 scroll/attach/save 回复的接收时机；drop/paste/composition 为合成事件，不认证系统输入法、Finder 拖放或系统剪贴板。
- 负向对照只修改临时副本：让程序化输入在实际写入时重新绑定当前附着。27 项生产 wiring 测试中，补全、ghost、清屏和文件的重附着用例全部失败（4 failed，退出 1），其余 23 项通过。正常实现的同组 27 项全部通过。没有修改工作树来运行该对照。

## 结果

| 检查 | 最终结果 |
| --- | --- |
| 完整 UI gate | PASS：837 passed，0 failed/skipped/cancelled；覆盖率 94.62% 行、89.96% 分支、87.86% 函数 |
| 定向输入与 smoke manifest 回归 | PASS：43 项；最后 smoke carrier 调整后另行复验 manifest/语法 |
| fmt / clippy / 模块 / JS 语法 / whitespace | PASS：退出 0；clippy 使用 workspace/all-targets/all-features/-D warnings |
| 完整 Rust workspace 重跑 | PASS：17 个测试套件，1,521 passed，0 failed，13 ignored（保留原有专用认证用例）；`cargo test --workspace --no-fail-fast -- --test-threads=1` 退出 0 |
| 隔离 debug 构建及来源 | PASS：sidecar、frontend staging、bundle provenance 均通过 |
| 真实 WKWebView terminal-input smoke | **BLOCKED**：锁屏运行不可验收；需要解锁后在新的隔离目录/socket 重跑全部 15 个检查点 |
| 隔离资源清理与最终 inventory | PASS：均退出 0，最终 apps/servers 均为空 |

代码修复与自动化门禁完成；原生验收未闭合。命令退出码、日志位置和 SHA256 见
[results.json](results.json)，锁屏运行的原始检查点见 [wk-locked.log](wk-locked.log)，
负向对照摘要见 [negative-control.log](negative-control.log)。

首次 `cargo test --workspace -- --test-threads=1` 退出 101：未改动的
`deck-lifecycle-helper/tests/cli.rs::a_host_that_never_closes_is_left_at_the_deadline`
观测到总耗时 1.079800416 秒，超过原有 1 秒上限。此时 PTY 边界用例及应用测试已通过。
检查确认计时涵盖子进程启动和交换，helper 本身仍使用原有 500 ms 总 deadline；
本轮没有改动该模块或放宽断言。该 CLI 完整 8 项定向复验退出 0。
计时超限的具体原因没有确证，不能将一次复验等同于修复了计时波动。

首次原生 smoke 未验收：系统 `CGSSessionScreenIsLocked=true`，Computer Use
无法取得该验收窗口（`cgWindowNotFound`），页面定时器明显延迟。
verdict 退出 1（2/15 检查点通过，滚动项失败，其余未执行）。该轮仅确认基础 PTY
读写与 IPC 观测可用；不能据此判定滚动或新边界的行为。精确 socket 的
`edr_runtime.py --cleanup --include-foreground` 退出 0，停止了本轮隔离应用与 cat/zsh
测试会话。已请求解锁，后续原生结果单独记录。

## 范围限制

附着代次只标识 PTY attachment，不证明同一附着内的前台程序未变，也不是 Agent
就绪或自动投递授权。这里治理的是本地人工输入的附着归属和顺序。

没有改变授权来源、scheduler/readiness、持久化 schema、Signal 信任、进程启动策略或签名更新后的 tmux 重启边界。未操作正式安装、真实 Agent、Mac mini、Windows、远端 CI 或发布渠道。此前复核保留的 re-entry shell 启动上下文窗口和 Pi 外部配置并发写入窗口不在此修复范围，本轮不宣称消除。
