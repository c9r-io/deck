# Phone Connector 隔离调查摘要（2026-09-29）

## 结果与证据

- 一键入口：`python3 scripts/connector-simulator-diagnose.py`。最终从零构建的默认运行报告：`/tmp/deck-connector-diagnose-owuvifyn/report.json`。本轮没有要求扫码、点击、登录、指定 Xcode 路径或选择 Simulator；用户人工介入 0 次。
- HEAD `3269891cbb2de5b46c9dd5321dba8ca96f1d3954`，源码版本 `0.7.19`；工作区含未提交的测试与诊断改动，未提交或发布。最终构建的差异 SHA-256 为 `3c9096f23730c6c6f1bd6dc43d51c54f25290f35957b916920bf40db67325a31`；host 二进制 SHA-256 为 `72621df0b33f1c7d7e033158e7d1c47bca89b7e43c4457dd3bf8dd7554beb9cf`，iOS App 为 `97e4136ac1a1ac3a6453a910f969192bc860800cdc4386a260aef41cb0e480b1`。
- Xcode `/Volumes/Yotta/Applications/Xcode.app`，`DEVELOPER_DIR=/Volumes/Yotta/Applications/Xcode.app/Contents/Developer`，Xcode 27.0（27A266a），iOS 27.0 runtime。两台专用 iPhone 17 Simulator：UI `FBD721B5-A9B8-4181-97B3-BE78BFFDFBBD`，app-hosted `00FF8220-E8C7-4CE4-83D9-5BA724515BAF`。
- 机器状态：`environmentReady=true`、`isolationVerified=true`、`reproductionConfirmed=true`、`requiredTestsExecuted=true`、`cleanupCompleted=true`；`productAssertionsPassed=false`，因为停止 session 的错误是稳定复现的待修缺陷。UI 主套件 3/3、Agent 退回 shell 的独立 UI 套件 1/1、app-hosted 1/1，均 0 failed、0 skipped；host smoke 两组通过。
- 证据：UI [report.json](/tmp/deck-connector-diagnose-8r1l0g1d/report.json)、[ui.xcresult](/tmp/deck-connector-diagnose-8r1l0g1d/ui.xcresult)、[shell-return.xcresult](/tmp/deck-connector-diagnose-8r1l0g1d/shell-return.xcresult)、[停止卡片截图目录](/tmp/deck-connector-diagnose-8r1l0g1d/screenshots)、[UI 日志](/tmp/deck-connector-diagnose-8r1l0g1d/ui-test.log)、[回到 shell 日志](/tmp/deck-connector-diagnose-8r1l0g1d/shell-return-test.log)；app-hosted [report.json](/tmp/deck-connector-diagnose-c_etk9_t/report.json)、[appmodel.xcresult](/tmp/deck-connector-diagnose-c_etk9_t/appmodel.xcresult)、[测试日志](/tmp/deck-connector-diagnose-c_etk9_t/appmodel-test.log)。原始上轮报告与 xcresult 仍在 `/tmp/deck-connector-diagnose-y74axmth/` 和 `/tmp/deck-connector-diagnose-rztl1pwf/`，未改写。

## 最小正反例与可信度

| 用例 | 本轮结果 | 可复核断言 |
| --- | --- | --- |
| A. 保存 `codex` 且真实 Codex 在前台 | PASS | Deck 正常 `createStarted` 创建一次性卡片与 tmux session；正常权限的真实 Codex 回复由分段输入组成的唯一标记；Simulator 正常详情布局经 pinned HTTPS 读取到该标记，而非只找到文本控件。仅认证了 Codex，未认证 Claude。 |
| B. 保存 `codex`，session 不存在 | PASS（缺陷复现） | 3 轮正常布局 UI 均显示完整 `Output unavailable: Deck host error: unavailable`；同一 fixture card 的 app-hosted 实际 HTTP 返回 `503` / `error.code=unavailable`。Rust fixture 与源码定位为 `probe-before` 的 `ErrorKind::NoSession`，尚未进入 history-size 或 capture-pane。 |
| C. Codex 已退出，session 保留且回到 shell | PASS | 自动核查同一 tmux session 的前台命令变为 shell，并生成仅在 shell 输出中的标记；Simulator 正常详情拒读 shell 输出，填写草稿后发送按钮仍禁用。此路径与 B 的 session 消失分开。 |
| D. 普通 shell 卡片 | PASS（现行契约） | Debug smoke 核查该卡片确已持久化到隔离 Board；app-hosted 通过同一 fixture 的 shell card ID 断言 host snapshot 不包含它。过滤点是 `snapshot_in` 的保存命令准入，不意味着已批准扩展 shell 远程权限。 |

旧 UI 用例只检查所有错误都共有的 `unavailable` 前缀，且诊断参数曾把输出区移到顶部。本轮移除布局分支，测试自动滚动正常详情；固定英文测试语言下断言完整文案，并以不同错误码的负向对照证明精确匹配。短效 descriptor 改为 host/iOS 构建、Simulator 启动和 App 安装完成后才生成。默认入口串行运行 UI 与 app-hosted 两组，各自使用新配对、host、Simulator 与 Keychain 生命周期；单套件选项仅供调试。

旧 `testOptInRealHostPairingBufferCASAndCredentialLifecycle` 的 `cmd=""`、`canQueue=false` 且仍要求读取 buffer 的前提已按当前保存 Agent 命令契约校准。陈旧 revision 可能同步返回 `.failed("revision-changed")` 或先返回 pending；两种路径都必须证明最终拒绝并且陈旧文本未写入，原有 buffer/CAS/Keychain 断言仍在。运行中确实出现过这两种时序，失败的 xcresult 均保留在对应私有运行目录。

前后台切换与 App 终止后凭据恢复已由三轮 UI 用例执行并分别记为 PASS；强制刷新失败与恢复、保存失败、备份恢复分别为 UNRUN，不能合并成通过。

## 根因边界与正式修复方案（未实施）

用户现场已确认的只有“桌面卡片多于手机、曾看见 shell 1、详情出现 unavailable”。没有真实 iPhone 的 hostId、该卡片保存命令、原始请求或底层 ErrorKind；本轮没有操作生产实例或建立生产配对。同一句 unavailable 不能证明现场与隔离复现同因，`deck_sessions_list` 也不是完整 Board。

1. **可见范围需单独产品决定。** 当前手机只投影保存命令通过 Agent 准入的卡片；标题和当前前台进程都不改变准入。若要让桌面用户理解差异，可在桌面标明 Connector 可见性及过滤原因。不要借此放开任意 shell 远程读写。
2. **区分状态失效。** host 对 `NoSession` 给独立稳定的 `session-stopped` 公开代码（建议输出 GET 用 HTTP 410），手机据此显示 session 已停止并保留 scratchpad；`canSend` 继续为 false。session 尚在而 Agent 退回 shell 时，使用另一个稳定代码（如 `agent-not-in-foreground`），手机明确说明 Agent 已退出；既不能读 shell，也不能给 shell 发送消息。保留前后 probe、MCP fence 与 generation 检查。
3. **保留真正读取失败的可诊断性。** history-size、capture-pane、probe-after 等阶段应由 host 记录最小结构化阶段、脱敏 card/request 关联及 ErrorKind，并映射稳定且不泄漏路径或终端内容的公开代码。Swift transport 保留 HTTP status/code 的结构化信息，AppModel/SwiftUI 依类别显示状态；不能只替换统一文案。刷新失败与旧 snapshot 的年龄另行标示，但本轮未验证该路径。
4. **协议与兼容。** 更新 Rust HTTP 映射测试、host/Swift golden error fixtures、Swift 错误分类与本轮 UI 验收断言。可以保持现有 snapshot 结构和认证、TLS pinning、控制权、generation 规则；旧客户端会把新代码显示为通用 host error，仍会安全拒绝发送。若将来新增 snapshot 状态或字段，先校对旧客户端解析和 `isStopped` 语义。

本轮范围外仍为 UNRUN：普通 shell 内手动启动 Agent、保存命令变体的 Simulator E2E、强制刷新失败与恢复、保存失败／备份恢复；真机 Keychain、Local Network、相机和真实网络仍需另验。历史 Rust 单元测试对其中部分路径的覆盖不能充当这些 E2E 用例通过。

## 收尾

最终两组 manifest 对应的 host PID 已退出、Simulator UDID 不再存在、私有 host 数据目录已删除、tmux socket 已不存在。复核本轮 31 个运行 manifest 时移除了此前 `kill-server` 后留下的无监听者测试 socket；入口已补充这一精确清理。复用的测试 Rust target 和 Codex 为一次性空目录写下的 6 条信任配置均已精确移除。保留的 `/tmp` 证据目录不含 pairing URI、token 或证书私钥；完整 xcresult 仅留在私有目录，未加入仓库。生产 Deck、真实 iPhone、用户现有 Simulator、卡片及配对均未被测试操作修改。

## 正式最小修复（同日后续，历史复现结论不变）

本节记录后续实施结果；上文是修复前调查，不能解释成当时产品已通过。修复实施时的基线 HEAD 为 `3269891cbb2de5b46c9dd5321dba8ca96f1d3954`，版本为 `0.7.19`；以下验证在提交前工作区完成。默认冷启动固定模式的[机器报告](/tmp/deck-connector-diagnose-wwfdswzn/report.json)记录源码差异 SHA-256 `097ecfdd84284d8403411f7b9dfdb2245c029eb9f9de2b3707b7ba90f34383b6`、host 二进制 SHA-256 `65befb66b8020935bee7047154c3cbd30e08992cf4bdd14240037952e3864ff5`、iOS App SHA-256 `f967fe05b9a3c200bce32f16558373f476452299ae8a67c0f7800195cadf1450`。Xcode 仍为 `/Volumes/Yotta/Applications/Xcode.app`（27.0，27A266a）；UI Simulator `539CFA27-EAFF-4F14-8990-6F1696870586`、app-hosted Simulator `AE2D90BD-B952-474D-BA57-3CE412D3F1C7`，runtime 为 iOS 27.0。

输出 GET 现在按实际阶段分类：初次探测确认 session 或 pane 不可用为 `503 session-unavailable`；存在 pane 但前台不是受支持 Agent 为 `409 agent-not-in-foreground`；读取期间目标消失或 generation 改变为 `409 context-changed`，已捕获文本仍丢弃；真正的 probe/history/capture 故障为 `503 output-read-failed`。缺失卡片、认证与 MCP fence 仍走原有拒绝路径。`pane_row` 对不存在的目标有时产生空行并归为 `Tmux`，因此 output 专用路径会用精确 `has-session` 和 pane listing 核实缺失，不解析或记录 tmux stderr。真实读取故障复用私有 app.log，记录阶段、ErrorKind、每进程脱敏 card tag、请求计数与耗时；正常不可用不重复告警。

Swift HTTP 层只对输出 GET 保留结构化 status/code，AppModel 映射成可恢复的卡片局部状态，SwiftUI 使用现有本地化方式显示并提供刷新；刷新重新取得 snapshot 后再读输出，不自动重发草稿。普通 shell 的投影与输出/输入权限、TLS pinning、认证、generation、MCP fence、command journal 的 operation ID/seq/410 expired 均未改变。旧 host 的 `503 unavailable` 在新客户端仍是未知读取故障；冻结旧解析逻辑的模型测试表明新 host 的 503 会降级为通用错误、409 会成为 conflict，均不扩大权限。兼容验证是协议 fixture、Swift URLProtocol 解码和冻结解析逻辑测试，不是历史 App E2E。

修复前新增 Rust 验收断言 `phone_output_and_send_are_limited_to_saved_agent_cards` 在旧行为上失败：实际错误消息为 `fixture`，预期 `session-unavailable`。首次完整隔离运行又发现真实 tmux 将部分不存在目标归为 `Tmux`，[失败 UI xcresult](/tmp/deck-connector-diagnose-qqprbf4l/ui.xcresult)与[脱敏 host 日志](/tmp/deck-connector-diagnose-qqprbf4l/host-app.log)保留了 `probe-before` 首次失败证据；这促成了精确存在性核查。最终固定模式的 UI 主套件 2/2、回到 shell UI 1/1、app-hosted 2/2，均 0 failed、0 skipped；两组 host smoke 通过。缺失 session 在正常详情布局三轮均显示新状态。真实 Codex 的输出标记对应其一次性 card；Agent 回到 shell 后的 shell 标记不可读且不能发送；持久化的普通 shell 卡片仍不在 snapshot。app-hosted 同一卡片核对 `503 session-unavailable`，并验证旧 unavailable 不会误判、后续成功输出清除错误状态。固定模式的 `productAssertionsPassed=true`、`requiredTestsExecuted=true`、`cleanupCompleted=true`、`reproductionConfirmed=null`（历史报告另存），人工介入 0。

最终证据包括 [UI xcresult](/tmp/deck-connector-diagnose-4jsfs_uk/ui.xcresult)、[回到 shell xcresult](/tmp/deck-connector-diagnose-4jsfs_uk/shell-return.xcresult)、[app-hosted xcresult](/tmp/deck-connector-diagnose-2b3nsf9u/appmodel.xcresult)、各运行目录中的脱敏 host 日志与截图。最终 manifest 中的 host PID、独立数据目录、tmux socket 和两台专用 Simulator 均已精确清理。原完整矩阵中的 shell 内手动启动 Agent、命令变体 Simulator E2E、强制刷新失败与恢复、保存失败及备份恢复仍为 **UNRUN**；现场 `shell 1` 仍无唯一归因。

补充验证：`cargo test --workspace` 退出 0（主程序 792 passed、2 ignored，工作区其余测试亦成功）；`cargo clippy --workspace -- -D warnings`、`scripts/ui-tests`、`node app/ui/js/check.mjs`、`cargo fmt --check` 与 `git diff --check` 均通过。Swift Core `swift test` 报告 62 项测试所在套件通过，隔离 host 专用项在普通命令中按预期 skip，已由上述 app-hosted 运行实际执行。随后使用更新过的入口单独运行 UI suite，[报告](/tmp/deck-connector-diagnose-7xiliuif/report.json)与两份 xcresult 显示 UI 2/2、回到 shell 1/1 均通过。该运行期间新建的精确 Codex 信任条目已由脚本自动移除，host、Simulator、数据目录及 tmux socket 也已自动清理。首次回归使用的额外 Rust target 已按[清理记录](/tmp/deck-connector-diagnose-wwfdswzn/extra-build-cleanup.json)删除。

[独立清理审计](/tmp/deck-connector-diagnose-wwfdswzn/cleanup-audit.json)复核了最终两组和后续单 UI 运行的精确 PID、UDID、目录及 socket，以及 Codex 信任条目。默认全套父 manifest 当时未同步写入聚合清理布尔值（其 report 与两个子 manifest 已记为完成，父级本身不拥有 host 或 Simulator）；脚本现已修正未来父 manifest 的这一字段，旧 manifest 原样保留并由审计解释。
