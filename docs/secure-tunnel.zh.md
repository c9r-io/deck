# 使用 Secure Tunnel 将 ChatGPT 连接到 Deck

让 ChatGPT 访问你明确授权的 Deck 项目，同时无需将 Deck 直接暴露在公网。

本指南面向使用 **macOS 版 Deck**、拥有 **ChatGPT** 账号及 **OpenAI Platform 组织访问权限**的用户。Secure Tunnel 适用于托管版 ChatGPT 这类无法直接连接本机 STDIO MCP 服务的远端客户端。如果只想让本机 Codex Desktop 使用 Deck MCP，请参阅 [Deck 的普通 MCP 设置](mcp.md#transport-and-clients)；这种用法不需要 Tunnel 或可选的 Helper。

完成本指南不需要 Kubernetes、公开托管 MCP、反向代理、入站端口、自己管理守护进程，也不需要了解 MCP 协议细节。

```text
Deck 设置 ── 控制 ──► Deck Tunnel Helper ── 启动/停止 ──► OpenAI 官方 tunnel-client
                                                                    │ 出站 HTTPS
ChatGPT ◄──────────────► OpenAI Secure MCP Tunnel ◄─────────────────┘
                                                                    │ 启动本机 STDIO
                                                                    ▼
                                                                 deck-mcp
                                                                    │
                                                                    ▼
                                                             你授权的 Deck 项目
```

Helper 负责设置和生命周期操作。**长期连接由 OpenAI 官方 `tunnel-client` 维护；它将 MCP 请求送到 `deck-mcp`。**Helper 不是网络流量的代理。

## 开始之前

- 安装支持 Secure Tunnel 集成的 Deck 0.7.8 或更新版本。**从同一份 [Deck Release](https://github.com/c9r-io/deck/releases) 下载 Deck 和 Deck Tunnel Helper。**如果已经安装 Deck，请先在设置中查看版本，再选择匹配的 Helper。
- Helper 是独立、可选、经过 Developer ID 签名和公证的 macOS App，不包含在 `deck.app` 内。没有它，Deck 和普通本机 MCP 仍可使用。它不会安装登录项、LaunchAgent 或常驻守护进程。
- 按照[官方安装说明](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md#macos-gatekeeper-blocks-a-downloaded-archive)安装 **OpenAI 官方 `tunnel-client`**。Helper 会检查受支持的官方程序；如果找不到，Deck 会显示 **tunnel-client missing**。较新的官方版本可能需要与之匹配的 Helper 更新。不要使用未知的分支版本或绕过 Gatekeeper。
- 你的 OpenAI Platform 组织必须允许你创建或使用 Tunnel，并创建组织级 Runtime API key。如果你不能创建 Tunnel，请让组织所有者或 RBAC 管理员创建，并提供 Tunnel ID。ChatGPT workspace 需要允许开发者模式连接，并与该 Tunnel 关联。参阅 [OpenAI 权限说明](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md)和 [Secure MCP Tunnel 指南](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)。

### 需要准备的三样东西

| 项目 | 从哪里获取 | Deck 如何使用 |
| --- | --- | --- |
| **Tunnel ID**（`tunnel_…`） | [OpenAI Platform → Tunnels](https://platform.openai.com/settings/organization/tunnels) | 标识与 ChatGPT 共用的远端 Secure MCP Tunnel。 |
| **Runtime API key** | [OpenAI Platform → Organization → Runtime API keys](https://platform.openai.com/settings/organization/api-keys) | 让官方 `tunnel-client` 使用该 Tunnel。Helper 在交互式设置中接收密钥；Deck 不接收。 |
| **Deck MCP client** | Deck 设置 → 远程访问 → MCP 终端控制 | 限定 ChatGPT 可以访问的项目、目录和操作。 |

通过 Platform 网页创建 Tunnel 时，**通常不需要 Admin API key**。Admin key 用于通过管理 CLI 管理 Tunnel；它与 Runtime key 不同，不能用于长期运行的 Tunnel 连接。

## 1. 创建 OpenAI Tunnel

1. 打开 [Platform → Tunnels](https://platform.openai.com/settings/organization/tunnels)，选择目标 Platform 组织。创建或编辑 Tunnel 需要 **Tunnels Read + Manage** 权限。如果无法执行，请联系组织管理员。
2. 选择 **Create tunnel**，起一个便于识别的名字，例如 **Deck — My Mac**，并选择正确的组织。如果希望在特定 ChatGPT workspace 中看到这个 Tunnel，请将该 workspace 纳入 Tunnel 的范围；组织管理员可能需要协助完成关联。
3. 创建后保存 `tunnel_…` ID。它是资源标识符，不是 API 密钥，但也不必公开。新 Tunnel 可能需要短暂时间才能出现在 ChatGPT 中。

[OpenAI Tunnel 指南](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)解释了组织与 workspace 的边界。仅在 Platform 组织中创建 Tunnel，并不能保证它会出现在某个 ChatGPT workspace 中。

## 2. 创建 Runtime API key

1. 在同一 Platform 组织中打开 **Organization → [Runtime API keys](https://platform.openai.com/settings/organization/api-keys)**。它与 **Project API keys** 和 **Admin API keys** 不同。
2. 给密钥命名，例如 **Deck Tunnel — My Mac**。选择 **Restricted**，只授予 **Tunnels Read** 和 **Tunnels Use**。仅用于运行 Tunnel 的密钥不要选择 **All** 或 **Manage**。
3. Platform 显示密钥时复制并妥善保管；它可能只显示一次。等到第 5 步 Helper 提示输入时再使用。

密钥的权限限制与**创建密钥的主体所拥有的组织/Tunnel 授权是两回事**。创建密钥的人或服务账号也需要对目标 Tunnel 拥有 Tunnels Read + Use；在 ChatGPT 中选择 Tunnel 的用户同样需要各自拥有这些权限。组织角色或用户组授予这些权限，单独设置受限密钥不能凭空授予访问权。参阅 [OpenAI 对角色与密钥的说明](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md#creating-keys)。

> **不要把密钥粘贴到 Deck、ChatGPT、shell 命令或配置文件中。**不要作为命令参数或环境变量传入。第 5 步由 Helper 的交互式输入提示接收密钥，并将它存入 macOS 钥匙串。

## 3. 安装 Deck Tunnel Helper

1. 从与你安装的 Deck **相同的 [Deck Release](https://github.com/c9r-io/deck/releases)** 下载 **Deck Tunnel Helper** 资产。解压 **Deck Tunnel Helper.app**，并将它移动到 `/Applications/Deck Tunnel Helper.app`。
2. 打开 Deck → **Settings → Remote access → MCP terminal control**。创建 MCP client 后（第 4 步），其条目会在 Deck 接受 Helper 时显示 **Secure Tunnel helper installed**。如果显示可选 Helper 缺失或不可用，请查看[排障](#排障)。

Release 中的 App 已签名并公证。无需 `sudo` 安装程序、`chmod`、删除 `xattr` 或绕过 Gatekeeper。安装 Helper 本身不会启用 Deck MCP，也不会授予项目访问权。

## 4. 授权 Deck MCP client

1. 在 Deck 中打开 **Settings → Remote access → MCP terminal control**。选择 **Enable**，阅读本机命令执行提示。
2. 选择 **Authorize MCP client…**，将 client 命名为 **ChatGPT**。明确选择 Deck **Project** 和项目内的 **Authorized directory**。如果项目已经配置目录，Deck 会预填；确认前仍要检查。请选择包含目标文件的最小目录。整个主目录范围过大，Deck 也会拒绝它。
3. 确认 Deck 显示的规范化目录。当 Deck 询问此集成能否创建可见的受管 session 时，如果只需结构化读取，选择 **Cancel**；确实需要创建 session 才允许。创建 session 是单独的权限，**并不授权执行命令**。

新 client 会出现在设置中。其 `client_…` ID 标识 Deck 授权，与 OpenAI Tunnel ID 不同。本流程不需要 **Copy config**；该按钮用于直接连接 STDIO MCP 的客户端。

## 5. 设置 Tunnel

在新 client 的条目中选择 **Set up…**。Deck 会将准确的 Helper 设置命令复制到剪贴板。打开可见的终端，粘贴并运行**这条命令**。命令只包含非机密的 Deck client ID，不包含 Runtime API key。

Helper 会依次提示输入 **OpenAI Tunnel ID** 和 **OpenAI Runtime API key**。分别在对应提示处粘贴；密钥输入不会显示。Helper 将密钥存入自己使用的 macOS 钥匙串区域；**Deck 不会收到密钥**。设置会创建本机 runtime 配置，**并立即尝试连接**，因此可能需要等待。回到 Deck 设置，重新打开或刷新 MCP 区域。设置成功后可能已经显示 **ready**，也可能显示 **stopped**。如果设置报错，请先查看[排障](#排障)，再决定是否重试。

## 6. 启动 Tunnel

如果 client 条目显示 **stopped**，选择 **Start Tunnel**。官方 runtime 建立到 OpenAI 的出站连接时，状态可能显示 **starting**。**Ready** 表示 Helper 已确认 OpenAI 控制连接正常；这还不能证明 ChatGPT 已发现工具。启动操作最多可能需要约两分钟。如果一直未进入 ready，请查看[启动一直停在 starting](#排障)。

如果设置后已经显示 **ready**，直接继续在 ChatGPT 中连接，无需先停止再启动。

## 7. 连接 ChatGPT

1. 在目标 ChatGPT workspace 中，如果账号和 workspace 允许，启用 **Developer mode**。Enterprise/Edu workspace 管理员控制此功能；开放时，用户可在 **Settings → Security and login** 中启用。参阅 [Secure MCP Tunnel 官方指南](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels#permissions-and-access)。
2. 打开 [ChatGPT Plugins / connector 设置](https://chatgpt.com/#settings/Connectors)。点击加号创建开发者模式 App。填写易识别的名称和说明，例如 **Deck on My Mac**。在 **Connection** 下选择 **Tunnel**，再选择已创建的 Tunnel，或粘贴其 `tunnel_…` ID。
3. 创建连接，检查 ChatGPT 发现的 Deck 工具及其说明，然后保存。新建对话，并从工具菜单选择该连接。工具发现和使用期间，保持 Deck Tunnel 处于 **ready** 状态。

[OpenAI 当前的 ChatGPT 连接说明](https://developers.openai.com/plugins/deploy/connect-chatgpt)展示了这个流程。如果选择器中没有 Tunnel，请查看[Platform 中可见、ChatGPT 中不可见](#排障)。

## 8. 用只读请求验证

在选择了新连接的 ChatGPT 对话中，先做无害的测试：

1. “检查 Deck MCP 连接，并显示我授权的 workspace。”对应 `deck_capabilities`。
2. “列出我的 Deck 项目中可以访问的文件。”对应 `deck_project_list`。
3. 请它通过 `deck_project_read` 读取授权目录内一个不含敏感信息的小测试文件。

这些结构化工具不会启动 shell。ChatGPT 显示的工具及确认界面可能受 workspace 策略和所选模型影响。

## 阅读文件与运行命令

Deck 的结构化 **list、read、search** 工具只在获准的项目目录内操作，不会启动 shell。如果你允许创建 session，ChatGPT 可以请求创建可见的 Deck 受管 session；但创建 session 仍不等于授予命令执行权。

`deck_exec` 还需要你在受管 session 卡片上进行单独的本机 **Approve execution…** 操作。你可以设定有时限的窗口（默认 15 分钟），并分别决定是否允许交互式 stdin 和共享作业输出。获批的命令以当前登录的 macOS 账号权限运行；**这不是操作系统沙盒**。你可以在本机接管可见 session，也可以随时撤销 client。连接 Tunnel **不会**自动给予 ChatGPT 命令执行权或整台 Mac 的无限制访问权。具体行为请参阅 [MCP 权限参考](mcp.md#execution-output-and-lifecycle)。

## 日常使用

- **启动：**打开 Deck 后，需要让 ChatGPT 远端访问时，在对应 client 中选择 **Start Tunnel**。只要 Runtime key 仍有效，之前成功设置过就无需重复设置。
- **停止：**不再需要传输连接时选择 **Stop Tunnel**。这不会撤销 Deck MCP client，也不会删除远端 OpenAI Tunnel。
- **重启后：** **Tunnel stopped 是预期状态。**Helper 有意不安装登录项、LaunchAgent 或后台常驻机制。需要时再次启动 Tunnel。仅退出 Deck 不一定会停止已经运行的 `tunnel-client`；请明确选择 **Stop Tunnel**。

## 停止远端访问与卸载

| 操作 | 效果 |
| --- | --- |
| 在 Deck 中 **Stop Tunnel** | 停止本机传输连接，保留 Deck client 授权。 |
| **Revoke** Deck MCP client | 立即撤销 Deck 权限，即使 Tunnel runtime 仍在运行。受管窗格中已经启动的程序可能继续运行，直到你在本机停止它。 |
| **Delete** 已撤销的 Deck MCP client | 删除授权记录和本机凭据。如果 runtime 仍存在，Deck 会提供 **Stop Tunnel**、**Delete Tunnel Runtime** 或 **Delete Deck Client Anyway**。本机 runtime 清理与 Deck 授权是不同的操作。 |
| **Delete Tunnel Runtime** | 删除本机 `tunnel-client` runtime 及 Helper 保存的密钥；不会删除远端 OpenAI Tunnel。 |
| 在 [Platform Tunnels](https://platform.openai.com/settings/organization/tunnels) 中删除远端 Tunnel | 删除 OpenAI 资源。Deck 不会自动执行。 |
| 在 [Platform Runtime API keys](https://platform.openai.com/settings/organization/api-keys) 中撤销 Runtime API key | 使该 OpenAI 密钥失效；不再需要时执行。 |

要移除可选的 Helper，先停止相关 Tunnel runtime，再删除 `/Applications/Deck Tunnel Helper.app`。Deck 和本机 MCP 仍可工作。完整清理还需分别撤销/删除 Deck MCP client、撤销 Runtime API key，并删除远端 OpenAI Tunnel。只删除 App 不会自动移除这些独立资源。

## 排障

| 看到的情况 | 检查方法 |
| --- | --- |
| **Optional Helper not installed** | 从同一 Deck Release 下载 Helper，安装到 `/Applications`，然后重新打开 MCP 设置。 |
| **Helper unavailable、untrusted 或 incompatible** | 从[官方 Deck Releases](https://github.com/c9r-io/deck/releases)重新安装已签名且版本匹配的 Helper。不要绕过 Gatekeeper。 |
| **tunnel-client missing** | 安装[受支持的 OpenAI 官方客户端](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md#macos-gatekeeper-blocks-a-downloaded-archive)。如果新安装的版本仍无法识别，请获取支持它的 Helper 版本。 |
| **`invalid_api_key` 或 401** | 确认密钥复制正确、尚未撤销，并且是**组织级 Runtime API key**。创建新的 Restricted 密钥，授予 Tunnels Read + Use，再运行 **Set up…**。 |
| **403 或权限错误** | 检查密钥本身的 **Tunnels Read + Use** 限制，以及所属主体的组织/Tunnel 角色；还要检查 Tunnel 所属组织。新角色授权最多可能需要 30 分钟传播。 |
| **Platform 中可见、ChatGPT 中不可见** | 检查 ChatGPT workspace 关联、连接用户的 Tunnels Read + Use 角色和 Developer mode 权限、新 Tunnel 的传播时间，以及 Deck 是否显示 **ready**。 |
| **启动一直停在 starting、unhealthy 或 stale** | 等待有上限的启动时间，再检查 Helper 与官方 `tunnel-client` 状态、Runtime key、Tunnel ID 和权限。参阅[官方排障指南](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md)。不要改用 Admin key。 |
| **重启后 Tunnel stopped** | 这是预期行为。再次选择 **Start Tunnel**。 |
| **`AUTH_REQUIRED`** | Deck MCP client 可能已撤销或删除，也可能是其本机凭据失效。请在 Deck 中重新授权；重启 Tunnel 不能绕过 Deck 授权。 |

## 安全模型

Deck 不持有 OpenAI Runtime API key；可选的 Helper 在交互式设置中接收它，并存入 macOS 钥匙串。官方 `tunnel-client` 向 OpenAI 建立出站连接，因此 Deck 不需要公开的入站 MCP 服务地址。Deck 自己的授权仍限制项目和目录、session 创建、本机执行许可、stdin 与输出共享、接管和撤销。Tunnel 连接不等于完整的 shell 权限。Helper 可以移除，macOS 重启后 Tunnel 也不会自动恢复。详细信任边界请参阅 [Helper 架构与安全参考](mcp-tunnel-helper.md)。

## 常见问题

**需要 OpenAI Admin API key 吗？**正常使用 Platform 网页设置时不需要。Admin key 用于通过 CLI 管理 Tunnel。

**Runtime API key 和普通 Project API key 是同一种吗？**不是。请创建组织级 Runtime API key，选择 Restricted，并授予 Tunnels Read + Use。

**Deck 会看到我的 Runtime API key 吗？**不会。只有 Helper 的交互式设置会接收它并存入钥匙串。

**ChatGPT 会获得整台 Mac 的无限制访问权吗？**不会。结构化访问受 Deck 授权的项目目录限制。命令执行还需单独、有时限的本机批准；批准后命令以你的账号权限运行。

**重启后 Tunnel 会自动启动吗？**不会。需要时点击 **Start Tunnel**。

**不用 Helper 也能使用 Deck 吗？以后能移除它吗？**可以。Helper 是可选的，本机 Deck MCP 仍可使用。

**多个 Deck client 能分别使用不同 Tunnel 吗？**Helper 按 Deck client ID 分别维护本机 runtime 和钥匙串条目。请分别创建、配置各自的 Deck client 和 OpenAI Tunnel；不要认为一个 client 的权限会延伸到另一个。

**多台 Mac 可以共用一个 Tunnel 吗？**当前没有为这种设置提供文档或推荐方案。除非 OpenAI 提供受支持的共享 runtime 设计，否则每台 Mac 使用独立的 Tunnel 和 Deck client。

## 进阶操作

上面的普通设置不需要管理 CLI。排查问题时，Deck 复制的命令中包含 `client_…` ID。可以用 `/Applications/Deck Tunnel Helper.app/Contents/MacOS/deck-tunnelctl status --client-id client_… --json` 查看该 client 的 Helper 状态。不要在命令中加入 Runtime key。设置后可以通过 **Copy Tunnel ID** 查看 `tunnel_…` ID。[OpenAI 官方 `tunnel-client` 操作指南](https://github.com/openai/tunnel-client/blob/master/docs/end-user-guide.md)介绍它自己的 doctor、status、权限与供 Tunnel 管理员使用的 admin CLI；其中通用的密钥处理示例并非 Deck 设置的必需步骤。[Deck Helper 参考](mcp-tunnel-helper.md)说明生命周期与安全边界。

## OpenAI 官方参考资料

- [Secure MCP Tunnel 指南](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
- [OpenAI Tunnel 管理与下载](https://platform.openai.com/settings/organization/tunnels)
- [官方 `tunnel-client` 用户指南](https://github.com/openai/tunnel-client/blob/master/docs/end-user-guide.md)
- [Tunnel 权限、角色与 Runtime API keys](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md)
- [连接并测试 ChatGPT Plugin](https://developers.openai.com/plugins/deploy/connect-chatgpt)
- [官方 `tunnel-client` 排障指南](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md)
