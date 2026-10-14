# Slack 频道监控配置治理与验收

日期：2026-10-15（Asia/Tokyo）；基线 `c53d3e7`。本轮仅修改工作树，没有发布或更新已安装 Deck。

## 诊断与实现

旧入口把缺少 Bot token、无权限等条件统一折叠为“需要升级”。已有的文字步骤没有定位既有 App 的入口，生成默认 Manifest 也不能保留用户自定义配置。Bot token 的 change 保存和完成按钮并行，立即点击可能读到旧状态；状态刷新还会清空正在输入的 token。

现在入口展开三个步骤：更新既有 App、重新安装并获取 Bot token、验证并启用。Deck 合并现有 JSON 的必要 bot/scopes/events，保留用户配置，复制准备好的 Manifest，并打开 Slack 设置或 OAuth 页面。已连接时从 Socket hello 识别 App ID，从 auth.test 获取工作区 ID；不具备完整导航信息时回退到 App 列表，允许手填 App ID。

凭据保存完成后才检查能力并写入启用设置；连续点击共享同一操作，保存失败保留草稿，取消可阻止尚未开始的启用写入。缺凭据、权限不足、凭据错误、工作区不匹配、网络/状态读取失败分别显示。启用后最多十次串行只读刷新连接状态，关闭引导停止。启用设置、凭据就绪、Socket 已连接与实际事件送达保持区别。

未增加凭据存储、Socket owner、权限来源或持久化 schema。现有规则、User/App token 与标记开关保留。产品仍需用户在 Slack 批准新增权限并将 Bot token 粘回 Deck；没有收集 Slack App 管理凭据或绕过授权。

## 真实沙盒验收

用户指定 Orchestrator FR114 Sandbox A 的 deck App，并完成 Slack 登录及一次浏览器前台恢复。测试仅使用 `deck-smoke.app`、独立临时数据目录和唯一 smoke socket。该模式的凭据走进程内存，不调用系统 Keychain；UI 的“已存入钥匙串”是共用生产文案，不代表验收写入了本机 Keychain。

已验证：

- 仅有真实 User/App token 的隔离实例点击启用，出现缺 Bot token 的具体三步引导。
- 从 Slack 复制现有 JSON，Deck 合并并复制回 Slack 编辑器；现有 bot scopes 和订阅已齐全，合并结果语义不变，Save Changes 保持禁用，没有修改沙盒权限。
- Manifest 与 OAuth 入口均打开正确工作区、正确 App 的实际设置页面。
- 粘贴真实 Bot token 后立即点击完成，先保存验证，随后启用；原标记开关保持开启，既有规则保留。
- 真实 Socket Mode 建立连接，频道及表情标记凭据状态就绪；清空手填 App ID 后仍能自动打开正确 App。

本轮没有发送 Slack 测试消息，没有把虚构 smoke 频道/用户 ID 改成真实对象，也没有启动 Agent。故真实消息匹配、频道成员资格、创建卡片与首步投递不作为本轮 PASS；对应合约由已有全量回归覆盖，不能替代新的真实消息验收。沙盒本身已具备所需权限，真实新增权限授权/重新安装为不适用；缺权限合并与拒绝路径由自动化回归覆盖。

## 验收中发现并修复的问题

1. i18n 的动态 key 没被静态检查识别：改为显式 key 映射后全量 UI gate 通过。
2. Rust 状态快照需要包含新增 App ID/工作区 ID：保留完整相等断言并更新 fixture，定向与全量回归复验。
3. 真实浏览器发现旧 `api.slack.com/apps/<id>/app-manifest` 链接返回 404：改为已验证的工作区/App 路径，真实页面复验通过；URL 回归覆盖缺失/恶意 ID 回退。
4. 连接已建立后设置页仍可能显示旧快照：补充有限次数串行刷新，并验证关闭后不再读取、不额外保存。

Slack 的 [Manifest 文档](https://docs.slack.dev/app-manifests/)说明了现有 App 的配置入口；本轮具体新版路由由真实浏览器页面验证。

## 最终门禁与清理

| 检查 | 结果 |
| --- | --- |
| 全量 UI gate | 826 passed，0 failed/skipped/cancelled；覆盖率 94.61% 行、89.97% 分支、87.83% 函数 |
| 新增设置流程回归 | 10 项通过：Manifest 保留/幂等、导航边界、状态分类、保存/点击竞态、失败重试、取消、自动刷新停止 |
| Rust workspace | 1,521 passed，0 failed，13 ignored（保留已有专用认证/子进程用例） |
| fmt / clippy | 退出 0；clippy 使用 workspace/all-targets/all-features/locked/-D warnings |
| JS 语法、模块、whitespace | 66 个 JS/MJS、63 个模块、git diff --check 通过 |
| 隔离 debug 构建 | sidecar、frontend staging 与 bundle provenance 检查通过 |
| 真实 WKWebView channel smoke | 3/3 检查点通过；新引导缺 Bot、错误 token、保留草稿、取消均包含在 channel-ui 中 |
| 真实 Slack | 已有权限的既有 App 路径通过；见上方实际覆盖范围 |
| 最终只读资源 inventory | 退出 0，无残留开发/验收资源 |

最终窗口无需关闭/重开或手工检查即更新为 Socket 已连接，成功反馈取得焦点并滚入可见区域，目视检查通过。四次隔离启动均已退出；清理仅通过对应完整 smoke socket 的 edr_runtime 验证入口。未触碰正式 Deck 的数据、Keychain 或服务；浏览器只保留原沙盒 App 页面，多余验收页已关闭，控制运行时的凭据变量已销毁。

命令退出码、日志路径与 SHA256 见 [results.json](results.json)；闭合计数见 [wk-channel.log](wk-channel.log)。没有执行远端 CI、签名发布/安装、Mac mini、Windows 或真实 Agent 认证。
