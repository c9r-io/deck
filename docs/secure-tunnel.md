# Connect ChatGPT to Deck with Secure Tunnel

Use ChatGPT to access a Deck project you explicitly authorize, without exposing Deck directly to the public Internet.

This guide is for a **Deck user on macOS** who also has **ChatGPT** and access to an **OpenAI Platform organization**. Secure Tunnel is for remote clients such as hosted ChatGPT that cannot directly use a local STDIO MCP server. For local Codex Desktop access, use [Deck's ordinary MCP setup](mcp.md#transport-and-clients); you do not need a Tunnel or the optional Helper.

You do not need Kubernetes, public MCP hosting, a reverse proxy, an inbound port, or your own daemon. You do not need to read the MCP protocol to follow this guide.

```text
Deck Settings ── controls ──► Deck Tunnel Helper ── starts/stops ──► official tunnel-client
                                                                    │ outbound HTTPS
ChatGPT ◄──────────────► OpenAI Secure MCP Tunnel ◄─────────────────┘
                                                                    │ starts local STDIO
                                                                    ▼
                                                                 deck-mcp
                                                                    │
                                                                    ▼
                                                       Your authorized Deck project
```

The Helper handles setup and lifecycle commands. **The official `tunnel-client` owns the long-running connection and carries MCP requests to `deck-mcp`**; traffic does not pass through the Helper as a network proxy.

## Before you start

- Install Deck 0.7.8 or later with Secure Tunnel integration. **Download Deck and Deck Tunnel Helper from the same [Deck release](https://github.com/c9r-io/deck/releases).** If you already have Deck, check its version in Settings before choosing the matching Helper.
- The Helper is a separate, optional, Developer ID signed and notarized macOS app. It is not embedded in `deck.app`. Deck and ordinary local MCP work without it. It installs no Login Item, LaunchAgent, or persistent daemon.
- Install the **official OpenAI `tunnel-client`** using the [supported installation instructions](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md#macos-gatekeeper-blocks-a-downloaded-archive). The Helper checks for a supported official binary; if absent, Deck shows **tunnel-client missing**. A newer official binary may require a matching Helper update. Do not use an unknown fork or bypass Gatekeeper.
- Your OpenAI Platform organization must allow you to create or use a Tunnel and create an organization Runtime API key. If you cannot create Tunnels yourself, ask an organization owner or RBAC administrator to create one and give you its Tunnel ID. The ChatGPT workspace must permit developer-mode connections and be associated with that Tunnel. See [OpenAI's permission guide](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md) and [Secure MCP Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels).

### Three things you need

| Item | Where it comes from | What Deck uses it for |
| --- | --- | --- |
| **Tunnel ID** (`tunnel_…`) | [OpenAI Platform → Tunnels](https://platform.openai.com/settings/organization/tunnels) | Identifies the remote Secure MCP Tunnel shared with ChatGPT. |
| **Runtime API key** | [OpenAI Platform → Organization → Runtime API keys](https://platform.openai.com/settings/organization/api-keys) | Lets the official `tunnel-client` use that Tunnel. The Helper receives it interactively; Deck never receives it. |
| **Deck MCP client** | Deck Settings → Remote access → MCP terminal control | Defines the project, directory, and actions ChatGPT may access. |

**You normally do not need an Admin API key** when you create the Tunnel in the Platform UI. An Admin key is for Tunnel management through the admin CLI. It is different from a Runtime key and must not be used for the long-running Tunnel connection.

## 1. Create an OpenAI Tunnel

1. Open [Platform → Tunnels](https://platform.openai.com/settings/organization/tunnels) and choose the intended Platform organization. You need **Tunnels Read + Manage** to create or edit a Tunnel. Ask an organization administrator if that action is unavailable.
2. Select **Create tunnel**, give it a recognizable name such as **Deck — My Mac**, and select the correct organization. Include the ChatGPT workspace in the Tunnel's scope if you want the Tunnel to appear in that workspace's connection picker. Your organization administrator may need to arrange the workspace association.
3. Create the Tunnel and save its `tunnel_…` ID. The ID identifies a resource; it is not an API secret, but avoid publishing it needlessly. Allow a short propagation period before expecting a new Tunnel in ChatGPT.

The [OpenAI Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) describes the organization and workspace boundary. Creating a Tunnel in a Platform organization alone does not guarantee that it appears in a ChatGPT workspace.

## 2. Create a Runtime API key

1. Open **Organization → [Runtime API keys](https://platform.openai.com/settings/organization/api-keys)** in the same Platform organization. This is separate from **Project API keys** and **Admin API keys**.
2. Create a key named, for example, **Deck Tunnel — My Mac**. Choose **Restricted**, then grant **Tunnels Read** and **Tunnels Use**. Do not select **All** or **Manage** for a runtime-only key.
3. Copy the key when Platform displays it; it may be shown only once. Keep it private until the Helper asks for it in step 5.

The key's restrictions and its **principal's organization/tunnel authorization are separate**. The person or service account creating the key also needs Tunnels Read + Use for the target Tunnel. The ChatGPT user selecting the Tunnel needs those permissions separately. Organization roles or groups grant them; a restricted key alone cannot grant access. [OpenAI explains the role and key split](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md#creating-keys).

> **Keep the key out of Deck, ChatGPT, shell commands, and configuration files.** Do not paste it into a command argument or environment variable. The Helper's interactive setup prompt receives it in step 5 and stores it in macOS Keychain.

## 3. Install Deck Tunnel Helper

1. From the **same [Deck release](https://github.com/c9r-io/deck/releases)** as your installed Deck, download the **Deck Tunnel Helper** asset. Extract **Deck Tunnel Helper.app** and move it to `/Applications/Deck Tunnel Helper.app`.
2. Open Deck → **Settings → Remote access → MCP terminal control**. Once you have an MCP client (step 4), its row will show **Secure Tunnel helper installed** if Deck accepts the Helper. If it says the optional Helper is missing or unavailable, see [Troubleshooting](#troubleshooting).

The release app is signed and notarized. There is no `sudo` installer, `chmod`, `xattr` removal, or Gatekeeper override step. Installing the Helper does not enable Deck MCP or grant project access.

## 4. Authorize a Deck MCP client

1. In Deck, open **Settings → Remote access → MCP terminal control**. Choose **Enable** and read the local code-execution warning.
2. Choose **Authorize MCP client…**. Name the client **ChatGPT**. Explicitly choose a Deck **Project** and an **Authorized directory** inside that project. Deck fills in the project's configured directory when available; review it before confirming. Choose the smallest directory that contains the files you want ChatGPT to use. The whole home directory is too broad and Deck rejects it.
3. Confirm the canonical directory Deck shows. When asked whether this integration may create visible managed sessions, choose **Cancel** for structured reading only, or allow creation if you need it. Session creation is a separate permission and **does not authorize command execution**.

The new client appears in Settings. Its `client_…` ID identifies the Deck authorization; it is distinct from the OpenAI Tunnel ID. You do not need **Copy config** for this Tunnel flow; that button is for direct STDIO MCP clients.

## 5. Set up the Tunnel

In the new client's row, choose **Set up…**. Deck copies an exact Helper setup command to the clipboard. Open a visible Terminal, paste **that command**, and run it. It contains the non-secret Deck client ID, not your Runtime API key.

The Helper prompts for **OpenAI Tunnel ID**, then **OpenAI Runtime API key**. Paste each at its own prompt. The key input is hidden. The Helper stores the key in its own macOS Keychain area; **Deck never receives the key**. Setup creates the local runtime configuration **and attempts to connect immediately**, so allow time for it to complete. Return to Deck Settings and reopen or refresh the MCP section. A successful setup can already show **ready**; otherwise it may show **stopped**. If setup reports an error, use [Troubleshooting](#troubleshooting) before repeating it.

## 6. Start the Tunnel

If the client row says **stopped**, choose **Start Tunnel**. It can show **starting** while the official runtime establishes its outbound OpenAI connection. **Ready** means the Helper confirmed a working OpenAI control-plane connection; it does not yet prove ChatGPT tool discovery. The start operation can take up to about two minutes. If it does not reach ready, see [Stuck on starting](#troubleshooting).

If setup already left it **ready**, continue to ChatGPT. You do not need to stop and restart it.

## 7. Connect ChatGPT

1. In the target ChatGPT workspace, enable **Developer mode** if your account and workspace permit it. Enterprise/Edu workspace admins control access; the user enables it in **Settings → Security and login** when available. See the [official Secure MCP Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels#permissions-and-access).
2. Open [ChatGPT Plugins / connector settings](https://chatgpt.com/#settings/Connectors). Choose the plus button to create a developer-mode app. Give the connection a recognizable name and description, such as **Deck on My Mac**. Under **Connection**, select **Tunnel**. Select the Tunnel you created, or paste its `tunnel_…` ID.
3. Create the connection, review the Deck tools and metadata ChatGPT discovers, and save it. Start a new conversation and select the connection from the tools menu. Keep the Deck Tunnel **ready** during discovery and whenever you use it.

OpenAI's [current ChatGPT connection instructions](https://developers.openai.com/plugins/deploy/connect-chatgpt) show this flow. If the Tunnel does not appear, check [Tunnel visible in Platform but not ChatGPT](#troubleshooting).

## 8. Verify with read-only requests

Start with a harmless test in a ChatGPT conversation using the new connection:

1. “Check the Deck MCP connection and show me the authorized workspaces.” This uses `deck_capabilities`.
2. “List the files available through my Deck project.” This uses `deck_project_list`.
3. Ask it to read a small, non-sensitive test file inside the authorized directory with `deck_project_read`.

These structured tools do not start a shell. ChatGPT's tool availability and confirmation screens can depend on workspace policy and the selected model.

## Reading files vs running commands

Deck's structured **list, read, and search** tools operate within the approved project directory without launching a shell. If you allowed session creation, ChatGPT may request a visible Deck managed session, but creating that session still does not grant command execution.

`deck_exec` requires a separate, local **Approve execution…** action on the managed session's card. You choose a limited time window (15 minutes by default) and separately decide whether interactive stdin and job output may be shared. Execution then runs with your logged-in macOS account permissions; **it is not an OS sandbox**. You can take over the visible session locally, and you can revoke the client at any time. Connecting ChatGPT to a Tunnel does **not** automatically give it command-execution permission or unrestricted access to your Mac. See the [MCP permission reference](mcp.md#execution-output-and-lifecycle) for exact behavior.

## Everyday use

- **Start:** After opening Deck, choose **Start Tunnel** for this client when you want remote ChatGPT access. A previous successful setup does not need to be repeated while its Runtime key remains valid.
- **Stop:** Choose **Stop Tunnel** when you no longer need the transport. This does not revoke the Deck MCP client or delete the remote OpenAI Tunnel.
- **After reboot:** **Tunnel stopped is expected.** The Helper intentionally installs no Login Item, LaunchAgent, or background persistence. Start the Tunnel again when needed. Quitting Deck alone does not necessarily stop an already running `tunnel-client` runtime; use **Stop Tunnel** explicitly.

## Stop remote access and uninstall

| Action | Effect |
| --- | --- |
| **Stop Tunnel** in Deck | Stops the local transport. The Deck client authorization remains. |
| **Revoke** the Deck MCP client | Removes Deck authority immediately, even if the Tunnel runtime is still running. A program already running in a managed pane may continue until you stop it locally. |
| **Delete** a revoked Deck MCP client | Removes its authorization record and local credential. If a runtime still exists, Deck offers **Stop Tunnel**, **Delete Tunnel Runtime**, or **Delete Deck Client Anyway**. Local runtime cleanup and Deck authorization are separate. |
| **Delete Tunnel Runtime** | Removes the local `tunnel-client` runtime and its Helper-held key. It does not delete the remote OpenAI Tunnel. |
| **Delete remote Tunnel** in [Platform Tunnels](https://platform.openai.com/settings/organization/tunnels) | Removes the OpenAI resource. Deck does not do this automatically. |
| **Revoke Runtime API key** in [Platform Runtime API keys](https://platform.openai.com/settings/organization/api-keys) | Invalidates that OpenAI key; do this when no longer needed. |

To remove the optional Helper, stop the related Tunnel runtime first, then delete `/Applications/Deck Tunnel Helper.app`. Deck and local MCP continue working. For a full cleanup, separately revoke/delete the Deck MCP client, revoke the Runtime API key, and delete the remote OpenAI Tunnel. Removing the app alone leaves those separate resources in place.

## Troubleshooting

| What you see | What to check |
| --- | --- |
| **Optional Helper not installed** | Download the Helper from the same Deck release, install it in `/Applications`, then reopen MCP settings. |
| **Helper unavailable, untrusted, or incompatible** | Reinstall the signed matching release Helper from [official Deck releases](https://github.com/c9r-io/deck/releases). Do not bypass Gatekeeper. |
| **tunnel-client missing** | Install the [official supported OpenAI client](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md#macos-gatekeeper-blocks-a-downloaded-archive). If a newly installed version is not recognized, obtain a Helper release that supports it. |
| **`invalid_api_key` or 401** | Confirm the key was copied correctly, is still active, and is an **Organization Runtime API key**. Create a new Restricted key with Tunnels Read + Use, then run **Set up…** again. |
| **403 or permission error** | Check both the key's **Tunnels Read + Use** restrictions and its principal's organization/tunnel role. Check the Tunnel's organization association. New role assignments may take up to 30 minutes to propagate. |
| **Tunnel visible in Platform but not ChatGPT** | Check the ChatGPT workspace association, the connecting user's Tunnels Read + Use role and Developer mode access, the Tunnel's propagation time, and Deck's **ready** status. |
| **Stuck on starting, unhealthy, or stale** | Allow the bounded start time, then check the Helper and official `tunnel-client` status, the Runtime key, Tunnel ID, and permissions. See the [official troubleshooting guide](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md). Do not substitute an Admin key. |
| **Tunnel stopped after reboot** | Expected. Choose **Start Tunnel** again. |
| **`AUTH_REQUIRED`** | The Deck MCP client may be revoked or deleted, or its local credential invalid. Re-authorize in Deck; restarting the Tunnel cannot bypass Deck authorization. |

## Security model

Deck never holds the OpenAI Runtime API key; the optional Helper receives it interactively and stores it in macOS Keychain. The official `tunnel-client` makes an outbound connection to OpenAI, so Deck needs no public inbound MCP endpoint. Deck's own authorization continues to govern the project and directory, session creation, local execution grants, stdin and output sharing, takeover, and revoke. A Tunnel connection is not full shell access. The Helper can be removed, and the Tunnel does not automatically restart after macOS reboot. For the detailed trust boundaries, see the [Helper architecture and security reference](mcp-tunnel-helper.md).

## FAQ

**Do I need an OpenAI Admin API key?** No for normal Platform UI setup. Admin keys are for Tunnel management through the CLI.

**Is the Runtime API key my normal project API key?** No. Create an organization Runtime API key with Restricted Tunnels Read + Use.

**Does Deck see my Runtime API key?** No. Only the Helper's interactive setup receives it and stores it in Keychain.

**Does ChatGPT get unrestricted access to my Mac?** No. Deck scopes structured access to the project directory. Command execution needs a separate, timed local approval and then runs with your account permissions.

**Does the Tunnel start after reboot?** No. Click **Start Tunnel** when needed.

**Can I use Deck without the Helper, or remove it later?** Yes. It is optional; local Deck MCP remains available.

**Can multiple Deck clients use separate Tunnels?** The Helper maintains a local runtime and Keychain entry per Deck client ID. Create and configure each Deck client and OpenAI Tunnel separately; do not assume one client's permissions carry over to another.

**Can I use one Tunnel from multiple machines?** This setup is not currently documented or recommended. Use a separate Tunnel and Deck client for each Mac unless OpenAI provides a supported shared-runtime design.

## Advanced

The ordinary setup above needs no admin CLI. For diagnostics, the command copied by Deck contains a `client_…` ID. You can inspect that client's Helper state with the signed executable in `/Applications/Deck Tunnel Helper.app/Contents/MacOS/deck-tunnelctl status --client-id client_… --json`. Do not put the Runtime key in this command. The `tunnel_…` ID is shown by **Copy Tunnel ID** after setup. The [official `tunnel-client` operator guide](https://github.com/openai/tunnel-client/blob/master/docs/end-user-guide.md) covers its own doctor, status, permissions, and admin CLI for Tunnel managers; its generic secret-handling examples are not required for Deck setup. The [Deck Helper reference](mcp-tunnel-helper.md) describes the lifecycle and security boundaries.

## Official OpenAI references

- [Secure MCP Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
- [OpenAI Tunnel management and download](https://platform.openai.com/settings/organization/tunnels)
- [Official `tunnel-client` end-user guide](https://github.com/openai/tunnel-client/blob/master/docs/end-user-guide.md)
- [Tunnel permissions, roles, and Runtime API keys](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md)
- [Connect and test a ChatGPT Plugin](https://developers.openai.com/plugins/deploy/connect-chatgpt)
- [Official `tunnel-client` troubleshooting](https://github.com/openai/tunnel-client/blob/master/docs/troubleshooting.md)
