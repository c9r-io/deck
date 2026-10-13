# Settings 状态反馈治理

日期：2026-10-14（Asia/Tokyo）。基线 `9a3b85556fdde3938fc460ccde5f58c83bc6cb91`。
本记录对应用户同意 UX 治理后的本地工作树，尚未提交、推送或发布。
文件哈希、退出码和本机日志见 [results.json](results.json)。

## 变更与产品边界

- Hooks 读取中、读取失败、字段缺失/非法值显示为未知：复选框 mixed、禁用，并提供说明与重新读取入口。
  Pi 的未知安装状态同样不显示成确定的关闭。读取等待最多 10 秒；过期和旧世代响应不覆盖当前状态。
  重新读取只调用 status；未知状态的处理函数也拒绝写入。原有启用确认继续保留。
- 每项普通设置及 Hooks 修改显示保存中、已保存、应用中、保存失败或已保存但应用失败。
  失败提示在关闭/重开设置及其它设置成功后保留；同一设置的新操作取代旧结果，也可手动收起提示。
  记录仅在本次应用进程内保留，不新增持久化 schema。关闭设置不取消写入、不要求额外确认。
- 侧栏以安静的数量提示保存问题；进入设置会定位到首个问题，底部也提供检查入口。
  行内反馈包含设置名称、结果与可访问的状态区域；中英文文案同步。底部说明改为自动保存、可随时关闭。
- 沿用上一轮字段级 edit 和唯一串行 writer；反馈不参与授权、grant、readiness、Agent 状态或工作完成判断。
  不改变 IPC 参数、Rust 存储、权限与发送语义。没有启用真实用户 Hooks，未操作生产应用或 Agent 配置。

键盘入口、投递恢复、注意力布局，以及其它 Settings 独立服务操作（如远程连接/资源安装）的统一反馈
不在本轮范围；本轮覆盖普通设置 writer 与 Hooks 开关，不声称所有设置操作都已统一。

## 验证

| 检查 | 结果 |
| --- | --- |
| 新生产 DOM 回归 | 7 项通过：保存/应用阶段、跨重开失败、同键结果顺序、重试、严格字段、超时/迟到响应、Hooks 写失败；IPC 为 mock |
| 完整 `scripts/ui-tests` | 811 passed，0 failed/skipped/cancelled；覆盖率 94.48% 行 / 89.90% 分支 / 87.56% 函数 |
| 语法 / 模块 / whitespace | 161 JS/MJS、62 模块、`git diff --check` 通过 |
| Rust 静态合约 | ipc_contract、edr_quiet、log_privacy、signal_census、session_architecture，共 55 passed |
| debug 构建与打包 | `app/run.sh` smoke 分支构建成功；5 个 sidecar、180 个 staging 文件、6 个 bundle 可执行文件来源复核通过 |
| 真实 WKWebView settings smoke | 最终工作树 7/7 检查点通过；英文/中文 × 100%/160% × 8 个设置区，共 32 组布局通过 |
| 真实 native 保存失败 | smoke 注入 settings-save 拒绝；确认值回滚、失败行保留、侧栏计数、关闭/重开定位；失败反馈存在时检查上述布局 |
| 目视与可访问树 | 隔离窗口中确认“检查更改”定位外观，失败文字/收起提示/底部操作区可见；未做 VoiceOver 人工朗读认证 |
| 清理 | 两次 smoke 均按精确 socket 经 edr_runtime 校验清理，最终只读 inventory 退出 0，无开发测试服务残留 |

最终 WK 记录见 [wk-settings.log](wk-settings.log)。复现入口为：新建 `/tmp/deck-settings-feedback-*`
目录，设置独立 `DECK_SMOKE_TMUX_SOCKET=deck-smoke-*` 和 `DECK_SMOKE_WKWEBVIEW=settings` 后
运行 `app/run.sh`；用 `scripts/smoke-verdict <目录> settings` 判定，通过同一精确 socket 的
`edr_runtime.py --cleanup` 回收。不要使用普通启动分支或生产 socket。

最初定向测试有 1 项失败：旧 Pi 测试要求读取失败后隐藏说明，与新要求冲突。
已改为断言未知说明，并增加 mixed 状态断言；未删除失败条件或弱化安全断言。
第一次 WK 已通过；随后补齐 Pi 单独未知时的重试入口和非法安装状态后，重建并重跑最终 WK。
完整 UI 门禁也在最后修改后重跑，以上结果只采用最终代码。

未运行完整 Rust workspace、fmt/clippy、远端 CI、正式签名安装/更新、Mac mini、Windows 或真实 Agent
集成验证。debug smoke 不能证明发布版 Hooks 安装效果，也不扩展既有上游 Signal 认证范围。
