# Settings 提交一致性治理

日期：2026-10-14（Asia/Tokyo）。基线 `ce872c19dcd007603f46352d5396666652e5302d`。
用户授权修复上一轮确认的 Settings 保存一致性问题。证据对应本地工作树，未提交、未推送或发布；
文件 SHA256 与命令退出码见 [results.json](results.json)。

## 问题与修复

旧 writer 串行保存整份乐观候选，回滚到另一份旧候选。串行落盘不等于修改彼此隔离：

- 主题失败、后续字号成功：失败主题被字号候选带入文件，内存却回退到旧主题和旧字号。
- 两次字号均失败：第二次回滚返回第一次尚未保存的字号。
- editor/locale 直接改 ctx.settings 后调用 persistSettings；失败既不回滚控件也不恢复有效语言，
  原生菜单还在落盘前更新。

settings.js 保留唯一串行 writer，每次操作捕获自己拥有字段的 edit，轮到写入时应用到最近成功的文档。
界面是该文档加所有仍在等待的乐观 edit；失败只撤去自己的预览。快捷键按 action 修改，翻译开关和
容量按各自字段修改，不把同一对象里失败的兄弟字段一起保存。未知的顶层扩展和快捷键扩展继续保留。

编辑器和语言通过 commitSettings；删除整份 ctx.settings 直接保存入口及其无用 app.js import。
既有两项测试改用真实字号选择占用 writer，保留原授权断言。语音仍在保存成功前不改变录音器，
但由共享提交路径更新设置，voice-settings 只保留控件锁、渲染和成功通知。

保存后的效果按 writer 顺序执行，接收该次实际落盘的值，不读取较新的乐观预览。
原生效果失败不会撤销已落盘选择，提示明确说“已保存，但更改未能完全生效”。普通保存失败则
恢复已保存值，不再错误使用“更新通道失败”的文案。共享控件的锁按未完成操作计数。

## 产品与授权边界

- 降低保存失败后重启、重复确认设置的成本，保留即时预览和真实失败反馈。
- committedInbound/nativeChannelSettings 的文件基线、原生 canonical grant、显式权限请求和
  proceed 的执行时点继续保留；普通设置不能携带未落盘的规则或恢复已撤销授权。
- 不改变 schema、Rust 权限/发送/恢复语义、IPC 参数、EDR 进程面或锁顺序。
- 不改 Board 退休、名称字符/字节上限，不引入新框架、全局类型迁移或通用状态管理库。

## 验证

新 `settings-write-dom.test.mjs` 直接调用生产 Settings 模块和 DOM handler，模拟 native 文件与可控 IPC gate：

1. 主题/字号、连续字号、不同快捷键，各自完整 2×2 成功/失败矩阵；检查中间预览、后续 payload、最终内存/控件/文件。
2. editor 失败回滚；locale 成功/失败、原生效果失败、两个排队语言选择的效果归属。
3. 未知扩展 round-trip。用例带 5 秒超时，afterEach 释放全部 gate，失败断言不能挂住后续测试。

反证：在隔离临时目录换回基线 settings.js/voice-settings.js，仅运行上述两项核心竞态回归，
Node 退出 1，2 failed / 0 passed / 0 cancelled。修复版本的完整门禁通过。

| 检查 | 结果 | 本机日志 |
| --- | --- | --- |
| 完整 `scripts/ui-tests` | 804 passed，0 failed/skipped/cancelled；覆盖率门禁通过 | `/tmp/deck-settings-ui-final.log` |
| 既有设置/首次保存/channel 竞态定向回归 | 100 passed，退出 0 | `/tmp/deck-settings-targeted.log` |
| 新 DOM 回归 | 18 passed，包含在完整门禁 | 同完整 UI 日志 |
| `node --check` / 模块检查 / diff whitespace | 160 JS/MJS、62 模块通过；未增加 view core 以外循环 | `results.json` |
| Rust 五组合约：ipc_contract、edr_quiet、log_privacy、signal_census、session_architecture | 55 passed，退出 0 | `/tmp/deck-settings-native-contracts.log` |
| `cargo build --bin deck-app` | debug 构建成功，未运行 app | `/tmp/deck-settings-build.log` |
| debug sidecar/main provenance / frontend staging | 通过 | `/tmp/deck-settings-provenance-final.log`、`/tmp/deck-settings-staging.log` |
| 基线反证 | 预期退出 1，2 failed | `/tmp/deck-settings-red.log` |

一次最初的新测试失败断言留住了模拟 gate；精确停止该测试进程后补上有界超时及 afterEach 释放，
后续运行正常退出。一次 provenance 检查漏传目标参数，退出 2；按工具用法补 `--binaries` 和
`--main-product` 后通过。这两项不计作成功检查，也未通过放宽产品断言处理。

本轮没有真实 WKWebView/native menu 验证，没有访问用户 settings、生产 app、Mac mini、Windows、
真实 Slack/Agent 或安装/签名/更新服务。完整 Rust workspace、fmt/clippy、release 构建、远端 CI
本轮未执行；前轮的结果不转写为本轮 PASS。新 DOM 用例证明前端事务与 IPC 调用顺序，不证明系统菜单实际显示。
