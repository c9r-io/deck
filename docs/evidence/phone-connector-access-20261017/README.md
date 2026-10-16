# Phone Connector：现有卡片的输出与直接交互

针对 2026-10-17 真机截图「sol worker → 输出不可用：仅共享概要」。只读核对时桌面已安装 0.8.3，该卡片保存的 `cmd` 为空。上一版解决了概要可见性，但仍把保存的启动命令同时当作输出、消息、便签、队列的准入条件；shell 内手动启动 Codex 的会话因此不能操作。

## 修复

Mac 的 Phone Connector 使用三个独立共享范围，旧配置不自动升级：

| 范围 | 普通/空命令卡片概要 | 输出 | 便签 | 直接消息 | 延迟队列 |
| --- | --- | --- | --- | --- | --- |
| 已保存的 Codex/Claude 卡片 | 不显示 | 保存配置＋前台 Agent | 保存配置 | 保存配置＋前台 Agent | 保存配置 |
| 全部卡片概要 | 显示 | 保持原限制 | 保持原限制 | 保持原限制 | 保持原限制 |
| 全部卡片输出和便签 | 显示 | 所有可核验的存活会话 | 包括停止卡片 | 当前前台 Codex/Claude，允许手动启动 | 仍需保存配置 |

第三项在 Mac 确认后适用于现有及今后卡片，明确包含 shell 的近期输出与历史；无需重启 Agent、修改卡片命令或重新配对。输出仍是有界终端快照，手机没有原始终端键盘、shell 输入或新的自动化权限。Pi 等其他程序可读输出，但本轮不扩大直接消息的 Agent 集合。

后端按动作独立准入。输出保留读取前后 pane/foreground generation、当前卡片和 MCP 控制检查。每个 GET 返回前复核设备、功能和共享修订号；撤销后立即恢复也不能让旧读取通过。共享范围变更使未决操作终结，恢复范围不复活旧操作。消息每次实际写入前在 lifecycle 锁下检查当前卡片、共享范围、执行中的原命令、设备、前台 Agent、generation 及 MCP 控制。普通清理仍可删除临时 paste buffer。

便签仍由唯一的前端 Board 事务队列写入；事务携带原 journal handle，通过功能注册的 opaque write-claim guard 在 native Board 落盘边界重新校验当前授权和 revision，锁顺序为 settings → Board → Connector lifecycle。documents 不反向依赖 Connector。iOS 修复便签可编辑性误用 `mayQueue` 的问题，并更新共享提示。队列的保存命令限制及所有外部自动化门禁保持有效。

进程检查是现有的进程名识别＋内核 PID/出生时间＋tmux 身份，不宣称二进制真实性或 Agent 输入就绪证明。

## 验证

最终结果记录在本目录的 `results.json`。 Rust workspace **1,537 通过、0 失败、13 原有忽略**；前端 **849/849**，Swift Core **80 通过、1 显式 opt-in 跳过**，experiment registry **30/30**；fmt、clippy、JS 语法/模块、扩展目录检查通过。最终 iPhone SE（iOS 27.0、深色）隔离 UI **5/5**，独立退出到 shell **1/1**，host WK smoke、构建来源检查与 registry 清理均通过，driver 退出 0。专项测试覆盖三种范围 × 保存命令 × 前台 Agent/shell × 存活/停止 × MCP 控制的能力矩阵；空命令输出与捕获期间目标/权限变化；便签与队列权限分离、CAS；撤权及重新授权不能恢复原操作；GET 返回前设备/范围/功能撤回；Rust/Swift 共用 `snapshot-board-access.json`。

隔离 UI 验收使用独立数据目录、临时目录内完整 tmux socket、一次性配对和模拟器。测试 shell 输出、实际便签保存/读回/删除，以及保存命令为空的真实 Codex 输出与手机直接发送。发送请求把回复标记分为两段，只有 Agent 的拼接回复包含完整标记。退出到 shell 后要求输出仍可读、发送禁用。进程与 tmux server 通过实验 registry 登记和回收。最终运行见 `native-final-report.json`，截图已目视检查：[手动 Agent 回复](manual-agent-reply.png)、[shell 便签写回](shell-note-saved.png)、[中文大字号横屏](chinese-large-text-landscape.png)。复验复用首轮隔离构建的 host target（生产代码未变，来源检查通过），重新编译 UI 测试；临时构建缓存已清理。

开发过程曾发现并修正两项测试夹具问题：能力矩阵错误地向 `golden_probe` 传入未知 session，导致模拟 probe 失败；扩充 debug transport 返回值后，有一个旧测试构造漏填 scope，编译拒绝。集成门禁还拦下了新增的 documents→Connector 依赖，已改成由功能注册的写入 guard，保留封闭依赖名单；新增的两处前台进程字段使用明确登记为 NotSignal，未增加 Signal 权限。外部输入 census 的文本匹配跟随 rustfmt 后的代码形式，并保留当前 scope、Agent 和 MCP 检查断言。断言未放宽。最初一次命令误用了不存在的 library target，未执行测试。

首轮 iPhone SE 模拟器 UI 验收为 3 通过、2 失败，退出到 shell 的独立测试通过，registry 清理通过。失败证据保存在 `native-first-report.json` 与 `native-first-ui.log`：新测试使用中心点重新聚焦后按删除键，没有清空草稿；草稿恢复测试点击后 AX 仍显示 Output 为 Selected，未进入 Notes。后续使用已存在的末尾光标探针验证后再删除，并在页面稳定可见后点击标签、断言选中状态，再定位 lazy List 的编辑器；保存、恢复、消息回复与 shell 禁止输入断言均保留。

未发布版本、未替换已安装应用、未修改生产卡片或配对。模拟器验收不代替真机 Wi-Fi/VPN、权限、锁屏/后台和 Mac 睡眠验证。用户已有的 governance 证据和 `research_notes/` 改动保留。

## 发布记录

用户随后明确授权提交并发布。修复及版本准备提交为
[`e41bf49`](https://github.com/c9r-io/deck/commit/e41bf49631d858b6838b63c2efb91bc2287ab3af)。
Mac 已发布 [0.8.4 Nightly](https://github.com/c9r-io/deck/releases/tag/nightly-v0.8.4-20261016-e41bf49)，
完整 CI、签名、公证、staple 和 Gatekeeper 检查通过；发布后重新下载全部产物，
验证哈希、Nightly 更新签名、commit/tag 及在线更新清单，确认 feed 指向 0.8.4。
iPhone **1.0 (3)** 已上传并完成处理，TestFlight「个人设备」组显示「正在测试」，中文测试说明已保存。
详见 [publication.json](publication.json)。未替换本机安装，也未代用户更新 iPhone。

发布前 14 个真实 WKWebView 场景最终全部通过。首轮综合场景被编辑器窗口遮挡，
`page-visible` 记录隐藏 197,568 ms；其余 13 场景通过。临时缩小编辑器窗口后，在新隔离根目录按原断言复验，
综合 94/94 通过、`page-visible a=1 b=0`；随后恢复编辑器窗口，实验资源按 registry 清理。
本地发布校验脚本首次因临时脚本导入名称错误停止；修正后完整校验退出 0，发布产物未修改。
