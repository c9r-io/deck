# Phone Connector 诊断证据

方案正文：[phone-connector-diagnosis-2026-10-16.md](../../phone-connector-diagnosis-2026-10-16.md)。本目录不包含配对 URI、token、私钥或生产终端内容。

## 直接观察

- [task-list.png](task-list.png)：隔离 Board 有一张普通 shell 卡、一个没有 session 的保存 `codex` 卡、一个保存 `codex` 的测试 Agent 卡。手机只显示后两张，不解释普通 shell 卡缺失；没有 session 的卡显示 `unknown`。项目内的列没有显示，也没有主机身份或同步时间。
- [stopped-detail.png](stopped-detail.png)：相同停止卡片的发送区显示无法确认目标，输出区显示没有可读会话。正文区域位于笔记、消息输入及多段说明之后。
- 原生 Device Hub 目视检查的 live card 首屏：笔记输入和说明占据大部分空间；正文在首屏下方。可访问性树能读到真实 Agent 回复标记，但这不是该文本在视口内可见的证明。
- [local-board-summary.json](local-board-summary.json)：本机生产 Board 只读、脱敏统计。5 张卡片保存命令全部为空，按当前源码的准入规则全部被过滤；没有核对用户手机的 host identity，不能将其当作唯一现场归因。

## 测试结果

| 运行 | 结果 | 解释 |
| --- | --- | --- |
| Swift Core | 61 passed、1 skipped、0 failed | [core-tests.json](core-tests.json)。未注入私有 fixture 的 macOS opt-in real-host 用例按设计跳过，不能算通过。 |
| 首轮 UI | 0 passed、2 failed、0 skipped | [initial-ui.json](initial-ui.json)。测试未找到预期卡片。与 app-hosted suite 同时运行；观察到 Xcode 替换 fixture 文件、host webview 再初始化等干扰，未证实唯一原因。 |
| 首轮 Agent → shell UI | 0 passed、1 failed、0 skipped | [initial-shell.json](initial-shell.json)。同一首轮实例，证据完整保留。 |
| 首轮 app-hosted | 1 passed、1 failed、0 skipped | [initial-appmodel.json](initial-appmodel.json)。真实 host 用例的原操作未在 15 秒测试期限内结束，后续编辑值检查也失败；独立输出错误恢复用例通过。不能把初次 POST 接受当作便签已保存。 |
| 串行 UI 复核 | 3 passed、0 failed、0 skipped | [serial-ui.json](serial-ui.json)。真实 Codex 唯一标记、停止会话三轮及前后台/进程重启、Agent 退回 shell 后拒读/禁发，全部执行。 |

串行复核保持相同产品源码和测试断言；临时诊断副本持续补回 Xcode 替换的同一私有 fixture，并取消测试辅助器对某一模型名称页脚的依赖。没有替换真实输出标记断言、修改错误文案预期或将 skip 视为通过。首轮失败没有被覆盖，也没有被归因为已修复的产品缺陷。

串行 app-hosted 复核 **2 passed、0 failed、0 skipped**：[serial-appmodel.json](serial-appmodel.json)。同一真实 host 路径完成 pairing、snapshot、buffer CRUD、旧 revision 拒绝、原操作恢复、Keychain 恢复与 unpair；另一个用例核对输出错误状态恢复。命令终态摘要见 [serial-command-states.json](serial-command-states.json)。首轮保存超时在本次串行运行没有复现，保留为未唯一归因的失败记录，不将其表述成已修复的产品 bug。

以上是特定契约的基线检查，**不是 UI/UX 验收通过**。前台内容动态更新、乱序响应、未保存便签离页/进程退出、离线恢复、同名/删除卡片、MCP 控制权提示及大字体等尚无本轮专项 E2E 结果；报告将它们标为代码确认或待验证。Device Hub 的 CUA 窗口连接后续报 `cgWindowNotFound`，因此未继续以手动操作冒充这些场景的复现。

## 可追溯性

- 原始证据保存在私有 `/tmp/deck-phone-audit-20261016/` 和各 JSON 指向的 `.xcresult`；没有把完整 XCTest 容器复制入仓库。
- UI 初轮：`/tmp/deck-connector-diagnose-epojcgfq/`；app-hosted 初轮：`/tmp/deck-connector-diagnose-lzh_4itt/`；串行 UI：`/tmp/deck-connector-diagnose-_56rum4z/`；串行 app-hosted：`/tmp/deck-connector-diagnose-kfs14u6f/`。
- 临时 runner：`/tmp/deck-phone-audit-20261016/diagnose.py`。由仓库的诊断脚本复制，只校准当前 Keychain 隔离检查、fixture 生命周期、测试辅助器和实验 registry；产品源文件 SHA-256 差异为空。原始工具故障与校准说明见 [notes.txt](notes.txt)。
- 宿主 bundle 的六个可执行文件已通过构建来源检查：[首轮](sidecar-provenance.log)、[串行](serial-sidecar-provenance.log)。
- 每批实验使用自己的 `TMUX_TMPDIR`，host 和 tmux 在启动时登记。只通过 `scripts/edr_runtime.py experiment cleanup --root …` 回收；每批报告记录 cleanup 结果。
- 本轮没有运行全仓 Rust/前端发布 gate：未修改生产源代码、协议或现有测试，只新增这份诊断文档与证据。实施修复时仍须执行 CLAUDE.md 的相关完整 gate。
- 真机相机、Local Network、Wi-Fi/VPN、锁屏 Keychain、长时间后台和 Mac 睡眠未测试。模拟器 loopback 成功不能证明这些场景正常。

最终资源复核见 [cleanup.json](cleanup.json)：本轮测试模拟器全部移除，现有 11 台模拟器保留且无 booted 实例；登记的 host/tmux PID 均已不存在，精确 socket 已移除，安装版 Deck 仍运行，构建缓存移至独立 Trash 目录。

清理命令本身全部成功。清理后再执行 registry show 时，四批记录因 runner 已删除 bundle 内的 tmux 程序而返回 3；已检查拒绝原因，没有绕过 registry 再停止任何进程。最终只读复核结合原始 registry stop 日志、全进程表中登记 PID 不存在、根目录无执行进程及精确 socket 不存在确认回收。未来 runner 应在删除 bundle 前完成 registry 复核，避免丢失复核工具。
