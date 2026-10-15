# 第一方插件自动注册与隔离验收

日期：2026-10-16（Asia/Tokyo）；基线 `fb12d0a`。本轮修改工作树，未发布、替换正式安装或修改生产更新源。

## 实现范围

三步已落地：声明式 Settings contribution 与共享状态宿主；从 `app/extensions/*.json` 自动生成 Rust 注册表和原生 ES 模块导入；Slack、Bergamot 的自定义面板和事件绑定接入相同目录。运行时挂载前端 UI，构建时发现受审查的第一方源码；不引入外部代码下载、运行时文件扫描、热卸载或新的授权来源。

后端按注册项生成 `agents` 状态映射，旧字段只投影同一次读取；启动迁移仍按原顺序更新已启用配置。Settings、Keychain、Slack transport、翻译安装和自动化授权仍由原模块持有。注册本身不会启用插件。

独立比较旧 HTML 与实际生成的面板：原有 397 个 ID 及其属性完整保留，无重复或缺失；新增的三个 ID 仅为挂载位置。设置分类、条目顺序、原有搜索标签和关键词均保持。原生中文设置和翻译截图已目视检查。

## 验证方式与隔离

完整命令日志位于 `/private/tmp/deck-extensions-20261016-002457/logs`；结构化汇总见 [results.json](results.json)。源目录、生成器和生成输出均进入构建来源校验。

测试使用独立 bundle ID、数据目录和 tmux socket。Hooks/Pi/通用 smoke 的 carrier、HOME、配置、临时文件及 tmux socket 全部位于各自 `/private/tmp` 实验根目录，进程和 server 启动后登记，结束只经 `edr_runtime.py experiment` 清理。翻译使用仓库已有的 checkout debug bundle 隔离驱动和精确 smoke socket 清理。没有运行普通开发启动分支，没有安装到 `/Applications/deck.app`。

Slack 使用用户指定的 Orchestrator FR114 Sandbox A 既有 deck App。通过 Brave 读取并在隔离窗口验证已有 User/App/Bot token；只保存在控制会话和测试进程内存，未使用生产 Keychain。完成后控制会话变量销毁。测试规则使用不会匹配真实对象的频道/发送者，未发送 Slack 消息、未启动 Agent、未改变 App 权限。网络/权限错误通过 debug-only 故障注入产生，没有改变本机网络。

Pi 使用本机已有 npm 运行时和本地测试模型，不访问用户的 Pi 配置或模型凭据。Claude Hooks 操作使用合成配置，检查两个 Hook 家族独立开关、拒绝确认无写入、重开状态和已有用户内容。

## 保留的初次失败与边界

- CI 工具清单最初拒绝新增 Python 测试不在统一 unittest 命令中；补入原门禁后全量 189 项通过。
- 扩大递归源扫描后，Signal census 首轮把两本词典既有的 `attention.bell` 标签计入消费者。现在精确登记两个标签，任何额外 `.bell` / `rang(` / `'bell'` 仍失败，没有豁免词典整文件。9 项定向及完整 Rust 覆盖率复验通过。
- Slack 首轮夹具没有对应项目，规则被原有孤立规则清理正确移除，未能建立连接。已经 registry 停止该实例，保留超时记录；补齐合成项目和模板的新实例通过 4/4 检查点，两个实例均清理成功。
- Hooks 原生 smoke 8/8 通过，但额外要求“启用再卸载后整个文件完全恢复”的临时断言失败：原先为空的 `Notification` 数组被安装填充，卸载最后一个自有 Hook 后该事件键被清理。基线与当前合并引擎逐字节相同（SHA256 见汇总），这是既有行为；除这个被操作的空事件集合，所有字节保持。按既有清理语义推导的完整最终文件与实测逐字节相等。原失败未删除、断言未放宽，也没有为本次注册改造改变卸载语义。
- Pi 布局夹具首轮只覆盖旧状态字段，新宿主正确地优先读取 `agents` 映射，预设场景因此超时。夹具现同时投影两种形状，原有布局、可操作性、未知状态与隐私断言保持不变。
- Signal-finish 的首次临时 carrier 没有携带其专用示例程序，原生入口正确拒绝；第二次补入示例后，其固定相对路径仍要求 carrier 名为 `deck-smoke.app`；重新核对入口契约并同时修正 carrier 名称和示例位置后，8/8 通过。示例始终放在该实验根目录，并由同一 registry 管理全部进程。没有改变产品入口或测试断言。

- 综合 UI 首轮在重命名停止：测试向已被替换的按钮查找输入框。修正为查找实时编辑节点后继续执行，原即时显示与持久化断言不变。
- 综合 UI 第二轮 92/94 通过，字号检查及汇总失败：快捷键写入只等到了内存提交，随后其应用回调覆盖了测试直接设定的 160% 字号。测试改为等待现有 `setFontScale` 设置事务，再进行原布局断言；没有修改产品逻辑或添加固定延时来掩盖竞态。最终综合 smoke 94/94 通过，原失败记录保留。

## 已执行门禁

| 验证 | 结果 |
|---|---|
| UI 单元与覆盖率 | 845 通过，0 失败/跳过；行 94.78%、分支 90.09%、函数 88.28% |
| Rust workspace 覆盖率 | 1,527 通过，13 原有 ignore，0 失败；行 85.67%、函数 83.78% |
| Python 工具 | 189 通过，包含 8 项目录生成器测试 |
| fmt / clippy、JS 语法与模块、声明新鲜度、workflow、tmux/Bergamot pin | 通过 |
| 原生 speech/notification 合约、剪贴板 guard、CE 本地完整校准 | 通过 |
| App / Tunnel audit 与 Tunnel fmt/clippy/tests | 通过；App audit 保留 8 条既有 informational warning |
| release 编译、翻译链接、EDR 扫描、本地 ad-hoc 兼容载体打包 | 通过；载体未安装/启动/发布 |
| 综合真实 WKWebView | 94/94 通过，包含输入/选区/布局/持久化等交互 |
| Settings / Channel / Connector / Approval / Empty-start / Signal-finish | 全部通过，细分检查点见结构化结果 |
| Slack 真实连接与故障恢复 | 4/4 通过，未发送消息 |
| 翻译 | 4 场景通过，含真实 HTTPS 安装和 Bergamot 翻译 |
| Pi npm / Pi 布局 | 各 16/16；布局含 48 组设置、6 组详情 |
| Claude Hooks | 原生 8/8；额外全文件往返断言按上述记录保留失败 |

## 最终隔离复核

正式 Deck 的进程 PID、启动时间和可执行文件路径与测试前完全一致；`~/.deck/settings.json` 和 `deck.json` 的 SHA256 与修改时间完全一致。所有本轮实验的 app/server 已经对应清理流程停止，资源残留数为 0。详见结构化结果中的 `production_comparison`、各场景 `cleanup` 和原始 inventory 日志。

## 范围限制

13 项 Rust ignore 保留原仓库的专用认证/子进程入口，不计作通过。真实 Slack 消息匹配、频道成员资格和消息投递未执行；这轮真实验证覆盖凭据、连接和恢复。未发布或执行生产升级；macOS 14 运行环境和远端 CI 不在本机，不能由本轮 macOS 原生结果替代。
