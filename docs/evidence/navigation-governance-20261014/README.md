# 键盘导航与注意力返回治理

日期：2026-10-14（Asia/Tokyo）。基线 `0743224fca2dbd8295d55501e84d2ab364cd70b4`。
本记录与实现一同提交；文件哈希、命令退出码及本机日志见 [results.json](results.json)。

## 行为与边界

- 卡片标题、侧栏会话、项目名称采用原生按钮，Tab 可达，Enter/Space 沿用既有打开动作。
  收起侧栏仍保留有名称的键盘入口；固定、关闭、提醒与状态说明保持独立。
- 卡片状态替换、看板/侧栏/项目重绘保留同一操作的焦点；目标消失时返回安全导航入口。
  仅在重绘区域原本拥有焦点时恢复，不从终端抢焦点。项目菜单关闭也回到原生项目按钮。
  行内重命名临时使用普通容器，避免将输入框嵌入按钮；完成后恢复原按钮。
- 注意力列表定位停止或失效卡片时，保存项目、筛选、滚动位置和原行，并聚焦卡片打开按钮。
  看板提供“返回待关注”；继续明确打开同一卡片后仍可返回原列表。另选项目或其它看板卡片
  清除该返回点，避免无关导航被带回旧上下文。
- 真实大字号验证发现徽标把标题压至零宽；标题与徽标现在各占一行，徽标行始终保留，
  状态变化不改变卡片高度。键盘焦点进入卡片时关闭图标可见。

定位和注意力入口不获得启动权限；有效会话仍用 `allowStart:false` 打开。用户明确激活普通
卡片/侧栏按钮保留原来的启动语义。没有新增持久化 schema、Signal 授权或自动移动行为。
投递恢复、注意力列表密度和 Agent 集成能力不在本轮范围。

## 验证

| 检查 | 结果 |
| --- | --- |
| 完整 `scripts/ui-tests` | 816 passed，0 failed/skipped/cancelled；覆盖率 94.44% 行 / 89.85% 分支 / 87.56% 函数 |
| 新增 DOM 回归 | 5 项：定位上下文、非注意力定位、有效会话禁止启动、重查失效、同卡片/无关卡片返回策略；依赖为 mock |
| 语法/模块/whitespace | 162 JS/MJS、62 模块、`git diff --check` 通过 |
| 五组原生合约 | ipc_contract、edr_quiet、log_privacy、signal_census、session_architecture：55 passed |
| 完整 Rust workspace | 1520 passed，0 failed，13 ignored（显式认证/基准及专用子进程用例，保留原有忽略） |
| Rust 格式与 lint | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过 |
| debug 构建及来源复核 | 5 个 sidecar、181 个 staging 文件、6 个 bundle 可执行文件验证通过 |
| 真实 WKWebView attention smoke | 51/51 检查点；新增 navigation 检查点含 18 个条件，失败位图为 0 |
| 布局 | 新入口中英文 × 100%/160% 共 4 组；既有注意力 locale/theme/font 12 组继续通过 |
| 原生键盘操作 | CUA 对隔离窗口发送真实 Shift+Tab、Enter、Space；确认项目切换并保留焦点，以及侧栏 Enter 打开本地 shell、焦点进入终端 |
| 清理 | 三次 smoke 均经精确 socket 的 edr_runtime 验证清理；最终只读 inventory 退出 0 |

WK 使用合成注意力观察和隔离空命令 shell，执行真实 DOM、原生 PTY attach 与返回动作；
定位路径和本轮匹配有效会话打开的新增检查保持 `starts === 0`。
这不是真实 Agent 状态认证，也不是 VoiceOver 人工朗读认证。
闭合计数日志见 [wk-attention.log](wk-attention.log)。复现时新建临时数据目录，设置
独立 `DECK_SMOKE_TMUX_SOCKET=deck-smoke-*`、`DECK_SMOKE_WKWEBVIEW=attention` 后运行
`app/run.sh`，再用 `scripts/smoke-verdict <数据目录> attention` 判定。
清理必须通过同一精确 socket 的 `scripts/edr_runtime.py --cleanup`；不使用生产 socket。

## 失败记录与适用范围

最初定向回归 40/42：两处旧静态断言针对被替换的 DOM 写法；更新后保留提醒隐藏、顺序和
事件隔离约束。初次 WK 有旧焦点/按钮计数断言失败，也有真实大字号布局失败。
更新原生按钮断言并加入失败位图后，第二次 WK 仅剩布局失败；目视确认标题零宽。
为标题和徽标分配独立空间后最终 WK 全部通过；布局断言分别检查滚动区内控件的所属行边界
和固定返回按钮的视口边界，保留标题最小可见宽度要求。最终完整 UI gate 在修复后重跑。

未执行远端 CI、正式签名安装/更新、Mac mini、Windows 或真实 Agent 集成测试。
本地 debug 结果不扩大历史 Signal 认证或 release 安装结论。
