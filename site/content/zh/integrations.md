# 可选集成

不启用任何集成，deck 也能当作本地终端使用。需要接入其他服务时，请在设置中分别启用，并先了解每种连接能读取或执行什么。

## Agent 状态

如果希望在「待关注」看到输入请求和本轮结束报告，可在「设置 → 集成与自动化」启用对应的 **Claude Code** 或 **Codex** 状态集成。deck 会在该 CLI 的配置中加入 hook；它只回报固定状态词和窗格标识，不传 prompt 或终端输出。[了解状态信号 →](/zh/guide/attention/)

## Slack 表情标记与频道监控

个人 Slack 连接可以在**你自己**给消息加上指定表情时触发自动化。在「设置 → 集成与自动化」配置连接，再到项目自动化中设置模板。触发时 deck 必须正在运行。[设置表情标记自动化 →](/zh/guide/automations/#从-slack-标记开始)

**Slack 频道监控**是另一条只读 Slack 连接，使用独立的 Bot 和 App token。项目规则需明确指定频道 ID、允许的用户或 Bot ID，以及文字匹配条件。第一条匹配消息会创建卡片并写入备忘录；后续匹配只增加笔记，不会自动发送给 agent。工作目录和裸 `claude` 或 `codex` 启动命令都来自已保存的规则。即使发送者在允许名单中，频道文字仍是不可信的 agent 输入；启用前请检查范围和模板。[阅读频道监控说明](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/channel-monitor.md)。

## 卡片备忘录

卡片备忘录用于保存该任务的笔记。你可以在本机查看、编辑，再决定哪些内容要加入 agent 队列。频道监控会把匹配消息写入备忘录，后续消息本身不会自动发送。已配对的手机也可操作符合条件的 agent 卡片笔记和队列。备忘录里出现一条文字，不代表 deck 已经把它发给程序。

## 手机 Connector

可选的手机 Connector 使用可直达的私有网络，将独立的 iOS 配套应用与 deck 配对。在「设置 → 集成与自动化」启用，检查配对二维码和设备列表。配对手机可以查看符合条件的 `claude` 或 `codex` 卡片最近输出、发送 prompt，也能按你在 Mac 上保存的预设创建任务。agent 可能据此以你的账号权限运行命令。不需要监听时可关闭 Connector；要终止某台手机的权限，应**撤销**该设备。它不提供公网中继，也不保证后台送达。[阅读手机 Connector 说明](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/connector.md)。

## Deck MCP 与 ChatGPT

**MCP 终端控制**默认关闭。你需要明确授权一个 client 的项目和目录。结构化的列举、读取、搜索不会启动 shell；允许创建受管 session 是另一个选项；执行命令还需在 Mac 上对该 session 给出有时限的独立批准。获批执行使用你的 macOS 账号权限，并非文件系统沙箱。可以在 deck 设置里撤销 client。[阅读 MCP 权限说明](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/mcp.md)。

本机 STDIO 客户端可以直接使用 Deck MCP。托管的 ChatGPT 需要另装可选的 Deck Tunnel Helper 和 OpenAI 官方 `tunnel-client`；OpenAI Runtime API key 不由 Deck 保存。[阅读 Secure Tunnel 完整指南 →](/zh/docs/integrations/chatgpt-secure-tunnel/)

## 分享问题报告之前

deck 的应用数据保存在本机；可选集成可能连接各自的服务。分享导出的日志、截图、待发送 prompt 或备忘录笔记之前，先检查是否含有敏感内容。[隐私说明 →](/zh/privacy/)
