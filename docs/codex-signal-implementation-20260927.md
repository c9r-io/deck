# Codex Signal 覆盖补充（2026-09-27）

承接 [诊断与方案](codex-signal-diagnosis-20260927.md)。本次落地缺失状态的可见提示、诊断入口及兼容路径说明，并保留独立的终端标题研究原型；没有把共享 daemon 的事件恢复成可信 Signal。

## 已实现

- 后端从现有 Signal target 和同一进程快照投影 `codex_signal`（unknown / trusted / unavailable），沿用已有前台 generation 和归属证据。不新增进程探测、持久化或权限来源。
- 没有可信观测的 Codex 卡片显示「Agent 状态未连接」或「Agent 状态不可用」，并提供「查看原因」。未知原因不会被说成已确认的共享 daemon 问题。
- 诊断解释卡片圆点仅代表终端活动，提供 hooks 设置入口、Codex `/hooks` 审查说明，以及新交互式 session 使用 `codex --no-daemon` 的可选兼容方式。
- 提示不会生成待关注事件、Dock 徽标或通知。真实状态到来后移除提示；过时快照保留过时标记；切换前台 generation 或停止 session 不继承旧证据。
- 不自动安装 hooks，不改写 Codex 配置或启动命令，不停止用户 daemon，不修改自动输入、卡片关闭或任务完成语义。
- [终端标题原型](codex-title-probe.md) 独立运行，仅研究严格封闭状态语法和缓存连续性，不接入产品的 Signal、提醒或自动化授权。

## 验证

| 检查 | 结果 |
| --- | --- |
| 完整 Rust workspace / all-targets / all-features 测试与覆盖率 | 907 通过，1 项原有 ignored；行覆盖率 83.92%，函数 82.20%，超过各 75% 门槛 |
| `cargo clippy … -- -D warnings` / `cargo fmt` | 通过 |
| 完整 `scripts/ui-tests` | 367 通过；行 93.65%，分支 86.29%，函数 83.44% |
| Signal consumer census | 6 通过；新增读取仅归类为 presentation |
| 隔离 WKWebView attention smoke | 47/47 通过，其中 Codex coverage 17 个断言；中英文、卡片高度、焦点保持/恢复、设置入口、旧快照、真实状态替换、无发送/导航副作用 |
| 实际界面目视检查 | 中文卡片缺口提示和诊断弹窗正常，未出现待关注徽标；最终中文句式精简后再次通过 i18n 测试 |
| JS 语法 / 模块检查，workflow actionlint，release/EDR/Signal tooling fixtures | 通过 |
| 完整 Python tooling fixtures（含标题原型） | 51 通过，其中标题原型 12 项；覆盖有界读取、超长输入恢复、重复字段撤销连续性与无换行末行 |
| Codex 0.157.1 真实隔离 shared daemon | 两个离屏 TUI 复用同一 daemon；无 `-c` override，重叠回合各自更新标题，本次未观察到串状态 |
| Codex 0.157.1 最小 embedded hook 对照 | `--no-daemon`：未信任 hooks 时零事件；仅实验使用审计后的单次 trust bypass 时收到 `working` / `turn-done`，均 v2 且 interaction 相同。没有据此宣称完整 Deck 接收链或权限/中断矩阵通过 |

Smoke 使用独立临时数据目录和 `deck-smoke-codex-coverage-*` socket；完成后仅清理本次创建的 app/tmux/shell，复查无残留。工作区原有 `CLAUDE.md` 修改保持不变。

## 边界

这些修改消除了「没有 Signal 却不告诉用户」的盲区，不等于已经恢复共享 daemon 下的完整信号覆盖。终端标题缺少可证明的事件身份、序列和完整回合边界，`Ready` 不能变成任务成功或 `turn-done`。运行探针的实际证据与尚未覆盖的矩阵记录在[探针文档](codex-title-probe.md)。

本次未构建或发布签名 nightly，也未完成安装包级 Codex 全功能候选认证。代码与隔离测试结果留在工作区，没有创建发布、tag 或修改 feed。
