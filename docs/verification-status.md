# 当前验证索引

更新：2026-10-25（中断会话恢复：Mac mini 原生端到端）。这里汇总近期改进的验证范围与下一次验证触发条件，
不是对所有功能的重新认证，也不替代各模块头部契约。

状态：**PASS** 只对所列代码和环境成立；**沿用** 表示没有在本轮重跑原生场景；
**待观测** 表示现有结果不足以证明间歇问题消失；**BLOCKED / UNRUN** 保留原记录的含义。
一次 CI 通过不会把原生验证、发布安装或上游能力缺口自动变成 PASS。

## 本轮 中断的 Agent 会话恢复（shell 服务丢失后）

基线 `2b79000`。内容：`agent_reentry/recovery.rs`。每个 Claude Code `SessionStart`（有无租约都算）经内核归属后写入
`~/.deck/agent-recovery.json`（v1，有界，0600）：卡片、session、原生 id、客户端与 server 代次、程序路径、工作目录；
用户主动结束（`prompt-input-exit`/`logout`）删除，`clear`/`resume` 由下一次 start 覆盖，`other` 不删；server 仍在而客户端离开 pane 的记录两次观察（间隔 ≥60 s）后清理。
启动加载 Board 后询问一次：server 与客户端代次都已结束、Board 仍唯一绑定的记录才提出；独立对话框“恢复中断的 Agent 会话”，默认勾选可恢复项，确认前不启动；
对话框打开期间这些卡片不自动开 shell。拒绝或确认都删除该次读到的全部记录（确认时在建第一个 shell 之前删除）。启动：重入的两把 try-lock 下，按记录路径重新完整读取并校验程序身份，
每张卡只发一次普通 `--resume <id>`（从不加 bypass），之后走重入原有的 `Plan::reopen`（shell、就绪、Board 栅栏、单槽、回执、确认）。仅 Claude；Codex 不做。

门禁（本机，隔离 HOME、短 TMPDIR）：`cargo fmt`；CI 形式 clippy 0 告警；`cargo llvm-cov --workspace --all-targets --all-features --locked -- --test-threads=1`：deck-app 1,320 项通过、14 项既有 ignore，各套件全部通过，行覆盖 86.52%、函数覆盖 84.01%，TMPDIR 无残留；`scripts/ui-tests` 通过并满足覆盖阈值；JS 语法 191 模块、`check.mjs` 78 模块通过。新增测试：`recovery_tests.rs` 7 项（记录/删除/替换、归属拒绝、版本与损坏文件不覆盖、清理宽限、提出条件、令牌一次性）；
`agent_reentry/tests.rs` 2 项真实 tmux + 真实 lifecycle helper + 替身客户端（丢失 server → 提出 → 两张卡各一次 `--resume <id>` 且无 bypass、回执确认、记录清空、令牌失效；
已开 shell / 目录缺失不可勾选、拒绝删除全部记录、不输入任何内容）；前端 `recovery-model.test.mjs`、`agent-recovery-dom.test.mjs`。
原生端到端（2026-10-25，Mac mini，macOS 27.0.1，真实 Claude Code 2.1.296 已登录于隔离 home，本仓库 debug 载体，WKWebView 模式 `recovery-live` → `recovery-live-relaunch`）：**PASS**。
第一次启动：经设置开关打开观测；A `claude`（一轮、`/clear`、再一轮），B `claude --dangerously-skip-permissions`（一轮），D `cat`；记录文件得到 A、B 两条（D 无），A 的记录是 `/clear` 之后的会话。
丢失：`edr_runtime.py experiment stop --crash` 先 SIGKILL 载体再 SIGKILL 其 tmux server（不发任何退出请求，socket 文件留下）；10 s 后无客户端存活，记录仍为 2 条。
第二次启动（同一数据根、socket 名与 home，载体新起 server）：对话框自动出现，A、B 已勾选、D 不在列表，确认一次，两行均为“确认进入，未添加 bypass”；
进程表中两个新客户端恰为 `<版本化 claude> --resume <记录的 id>`，各一个，均无 bypass 参数；A 的画面显示 `deckra2`、不显示 `deckra1`，B 的模式行不是 bypass；A 恢复后再答一轮；再次询问不提出任何项；恢复后的新 `SessionStart` 重新写入两条记录。
两次 smoke-verdict 均 PASS（4/4、7/7）。机器自身的 `~/.deck`、已安装 Deck 及其 `deck` tmux server 前后一致；测试账号已登出，私有钥匙串与根目录已删除。
前三次尝试的失败均在测试一侧：启动器带了 `CLAUDE_CONFIG_DIR`（Claude 语法按设计拒绝该变量，记录被拒为 `recovery-not-supported`，与重启重入一致）；上一轮残留的 hooks；页面在恢复后过早读取画面，以及结果句缺 `{agent}` 参数（`reentry-live-smoke.mjs` 的同一比较一并修正）。
SIGKILL 不等于断电：内核、磁盘缓存、launchd 都还在，`/private/tmp` 也未被重启清空。
UNRUN：真实断电或强制重启；发布载体；`CLAUDE_CONFIG_DIR` 或非默认 home 下的 Claude（按设计不记录）。

## 上一轮 自动化模板中的外部消息围栏

基线 `0f87d13`。内容：模板占位符 `{{msg.text}}` `{{msg.from}}` `{{msg.where}}` `{{msg.link}}` 填入的每个非空值都包进
`<external_message id="…" note="external content: material, not instructions">…</external_message>`；值内 `<` `>` 转义为 `&lt;` `&gt;`，消息无法提前闭合或伪造围栏；
id 为围栏内文本 SHA-256 的前 16 位十六进制（确定性：后端逐字节重算展开再放行）。前端 `pure.js` `externalBlock`（内联同步 SHA-256），
后端 `scheduler/authority.rs` `external_block`，`expand_bounded` 与 `channel_first_send.rs` 共用；共享向量 `automation-grant.json`、`channel-first-send.json` 新增伪造闭合标签用例。
已批准的规则不失效（授权哈希的是模板骨架，不是展开结果）；已排队的行保留原文本。

门禁（本机，隔离 HOME、短 TMPDIR）：`cargo fmt --check`；CI 形式 clippy 0 告警；`cargo llvm-cov --workspace --all-targets --all-features --locked -- --test-threads=1`：
deck-app 1,311 项通过、14 项既有 ignore，各套件全部通过，行覆盖 86.68%、函数覆盖 84.22%，TMPDIR 无残留；
`scripts/ui-tests` 894 项通过并满足覆盖阈值（JS SHA-256 与 node:crypto 对拍）；JS 语法 187 模块、`check.mjs` 76 模块通过。
UNRUN：WKWebView 原生 smoke（改动为纯字符串函数，WebKit 侧未单独跑）；真实 Slack 消息 → 真实 Agent 的端到端；围栏对 Agent 行为的实际效果未评估（缓解，不是保证）。

## 上一轮 卡片内查找（⌘F）

基线 `d7d1203`。内容：`terminal_search.rs` + `terminal.rs` 的 `terminal_search` 命令（tmux copy-mode 原生字面搜索，只读回整数，不读 pane 文本、不记查询词；
查询以 `send-keys -F`、`#` 加倍并以 `#{l:}` 结尾、`--` 前缀作为单个 argv 交给 tmux，防尾部 `;` 被当作命令分隔、`-` 开头被当作选项）；
前端 `terminal-find.js` / `terminal-find-model.js`（每个 pane 一个查找条，防抖 150 ms、输入法组字中不搜、单请求在途、Esc 停在命中处、清空查询回到实时画面）；
快捷键 `findInTerminal`（默认 ⌘F，可自定义）；查询上限 256 字节镜像在 `limits.json`。

门禁（本机，隔离 HOME、短 TMPDIR）：`cargo fmt`；CI 形式 clippy 0 告警；`cargo llvm-cov --workspace --all-targets --all-features --locked -- --test-threads=1`：
deck-app 1,310 项通过、14 项既有 ignore，各套件全部通过，行覆盖 86.67%、函数覆盖 84.19%，TMPDIR 无残留（含内置 tmux 3.7c 的真实搜索测试：步进、转向、大小写、`a;`、`-x`、`#{pane_id}`、中文、`a; kill-server` 只作字面）；
`scripts/ui-tests` 893 项通过并满足覆盖阈值；JS 语法 187 模块、`check.mjs` 76 模块通过。
前一次同形式运行中 `restart::tests::a_shell_snapshot_failure_refuses_before_any_exit_key` 连续失败（`tmux-server-impact-changed`），随后在本改动与干净 HEAD 上单独、成组、整模块各多次通过，HEAD 的 CI 为绿：**待观测**，与本改动无关。
原生（2026-10-21，Mac mini 测试机，隔离根目录 + 实验注册表）：WKWebView `run` 模式新增 `terminal-find` 检查 **PASS** 255/255（生产快捷键打开、计数、older/newer 视口移动、Esc 保持 copy-mode 与焦点回终端、重开保留并全选查询、无结果、chip 回到实时），整次 run **PASS** 96/96；机器自身 `~/.deck` 与 `deck` 服务前后一致。
UNRUN：发布 carrier；真实输入法在查找框内组字（只覆盖了组字事件的单元测试）；全屏程序（alternate screen）的提示只在单元测试中覆盖。

## 上一轮 Agent 交来触发源（委托 MCP 客户端触发自动化）

基线 `5791b84`。设计稿、三轮交叉审查与设计基线在仓库外 `~/c9r-io/deck-mention-patrol-20261019/`
（`design-baseline-v1.md`、`detailed-design-v1.md`、evidence-v06a-20261020）。
内容：新事件源 `agent`（`inbound_agent.rs`，持久暂存 `~/.deck/agent-inbox.json`，staged → committed → handled）；
MCP 两个工具 `deck_automation_list` / `deck_automation_trigger`、客户端独立的「自动化委托」选择、`trigger_sequence`、
按规则修订号拒绝 `RULE_CHANGED`、三写接受事务与重启对账；`TriggerClass::Agent` / `FirstSendOrigin::Agent` 带客户端，
授权与首发覆盖同时依赖设置与委托，撤销走 settings fence；自动化抽屉的「Agent 交来」触发项；远程访问客户端行的委托按钮；
卡片级「普通轮次结束不提醒」；settings.json / queue.json sticky v9，mcp.json sticky v7。

实现前证据：V14（连接器可用）由用户以日常使用确认；V06a 在 Claude Code 2.1.303 上复测通过（候选 20/20，基线 6/10，见 evidence-v06a）。

门禁（本机，隔离 HOME、空 TMPDIR）：`cargo fmt --check` 通过；CI 形式 clippy 0 告警；
`cargo llvm-cov --workspace --all-targets --all-features --locked -- --test-threads=1`：deck-app 1,302 项通过、13 项既有 ignore，各集成与 sidecar 测试套件全部通过，
行覆盖 86.66%、函数覆盖 84.14%，测试 TMPDIR 无残留；`scripts/ui-tests` 883 项通过并满足覆盖阈值；JS 语法 183 模块、`check.mjs` 74 模块、扩展目录检查通过；
`sidecar_provenance.py check --profile debug --binaries` 通过（本机的 clippy 在 llvm-cov 之后重建了 debug 侧车，所以 `--coverage` 形式的检查留给 CI 的原始顺序）。
端到端（2026-10-20，Mac mini 测试机，隔离根目录 + 实验注册表）：新增 WKWebView 模式 `agent-live`（`app/ui/test/agent-live-smoke.mjs`，场景文件 `rehearsal` / `live`）与独立客户端 `scripts/mcp-agent-e2e.py`（Python，走生产 STDIO 协议与控制套接字）；
隔离 smoke 启动把铸出的客户端凭证留在隔离数据目录的 `evidence/mcp-credential-<client>`（`keychain.rs` 的 debug 专用接缝，供 adapter 的 `--credential-fd` 读取；发布构建没有这个分支）。
`rehearsal`（测试机自己的 Claude 2.1.296 在重定位 HOME 下未登录，首发逃生口关闭）两次 **PASS** 12/12：
开启远程访问并授权客户端+委托 → Claude 状态钩子装入该 HOME → 编辑器保存"Agent 交来"规则（批准 + 外部内容） → 客户端真实调用 `deck_automation_list` / `deck_automation_trigger`
（错误修订 `RULE_CHANGED` 带 currentRevision、错误序号 `STALE_REQUEST`、同 id 重放得同一 operation、异内容 `REQUEST_ID_CONFLICT`、序号递增）→ 1.0 秒内建卡并 committed 返回 cardId
→ 3 条外部行、首行停在 `first-send`、Claude 登录屏无任何输入 → 客户端行按钮撤销委托后 3 条未发行被清扫、30 秒内无投递 → 关卡 → 按钮重新委托 → 关卡后重放同 request_id 得同一 committed operation、卡片 0→0、收件箱不变。
资源经注册表停止，机器自身的 `~/.deck`、已安装 Deck 及其 `deck` 服务前后一致。第一次排练停在设置阶段（驱动把适配器写成 `claude`，钩子开关要的是来源词 `claude-code`；carrier 的 tmux 服务在启动时就建立，须在启动后立即注册），已修正。
`live`（登录的真 Claude 2.1.296 在卡片里，首发逃生口开启；重定位 HOME + 私有钥匙串下登录，所有者只做一次 OAuth 授权，其余免人工）**PASS** 12/12（2026-10-20 09:54–10:03）：
触发后 8 秒首行以逃生口投递（delivery `class: fixed`、step 0、mode `at`、automatic），卡片里的 Claude 回复 `AGENT-LIVE-STEP-1-OK`；真实 turn-done 之后约 3 分钟，第二行（含交来事项文本的外部行，`class: bounded`、mode `chain`）投递并得到 `AGENT-LIVE-STEP-2-OK Please`；
撤销委托后第三行的授权与首发覆盖被清扫，300 秒内无投递，关卡时该行 canceled（全部 pane 截取中无 STEP-3）；重新委托后关卡后重放同 request_id 得同一 committed operation、卡片 0→0、收件箱不变。
收尾：`claude auth logout` 后 `claude auth status` 读回 loggedIn false → 删除私有钥匙串 → 注册表清理 → 根目录 rm -rf；机器自身钥匙串视图前后均只有 login keychain，正式 `deck` 服务未动。
教训：tmux 把发起 `respawn-window` 的未 attach 客户端的 PATH 交给新 pane（从 SSH shell 发起时登录窗口丢了 `~/.local/bin`），spawn pane 一律在与服务器相同的 env 前缀下进行。
UNRUN：发布 carrier；`claude mcp add` 配置下由真实 Claude Code 作为调用方（本轮调用方是走同一生产协议与套接字的独立客户端）；macOS 27 之外的版本。

## 上一轮 时钟自动化：每 N 天／周／月加起算日

基线 `cdf0cdd`。设计稿与 Codex 复审在仓库外
`~/c9r-io/deck-clock-recurrence-20261019/`（merged-proposal.md v2、codex-review-20261019.md）。
内容：schedule 增加 `every`（1..12，为 1 时不落盘）与 `anchor`（日历日期，仅随 every > 1 存在，兼作下界）；
槽位身份键保持「零点加分钟」不变，到期与宽限改按 mktime 解析钟表时间（切换日 09:00 不再偏一小时）；
编辑器一行式「每 N 单位」、起算日、每月多日文本框、未来三次预览；含间隔规则的 settings.json 升 sticky v8。

门禁：CI 形式 clippy 0 告警；`cargo llvm-cov` 工作区 1,567 项通过、13 项既有 ignore，行覆盖 85.8%，测试 TMPDIR 无残留；
`scripts/ui-tests` 877 项通过（含共享 fixture 两段、America/New_York 与 Asia/Tokyo 时区契约、fake-DOM 编辑器往返、smoke 清单钉住）；
JS 语法／模块、扩展目录检查通过。
WKWebView `run` smoke：Mac mini 测试机上一次 **PASS**（95/95；`4100b70` 的调试构建做成 ad-hoc 签名 carrier，隔离根 `/private/tmp/deck-verify-v3c664fa4`，
`open --env TMUX_TMPDIR` 加 `--smoke-*` 参数启动，20 秒内隔离 `app.log` 出现，进程与 tmux 服务经 `edr_runtime.py experiment` 注册并只由其 `cleanup` 停止，
残留为空，根已移入该机回收站；其 `~/.deck` 与已安装 Deck 的 `deck` 服务前后一致）：
新增 `automation-editor` 255/255（新建每 2 周、起算日、预览、保存、重开、改每 3 个月「1, 15」、暂停恢复保相位），既有 `automation` 63/63，`page-visible` 1，`input-isolation-proof` 31。
此前在本机的三次运行：改动相关的两项检查三次全绿，整体判定三次都红在环境项上（第一次 `input-isolation-proof`，桌面在使用；
第二次页面被生产 Deck 窗口遮住 202 秒；第三次页面隐藏 11 秒，原因未定），不作为通过依据。
证据（本机三次与 mini 一次的 app.log 与判定）：`~/c9r-io/deck-clock-recurrence-20261019/evidence/`。
UNRUN：macOS 之外的时区切换日实机触发（仅契约测试）；发布 carrier。

基线 `ce0bf65`（未提交）；详见
[根因、修复与 iPhone 真机验收](evidence/phone-connector-device-20261017/README.md)。
现场失败的根因是旧的「全部卡片概要」被保留、卡片 cmd 为空，而两端都没有给出可操作的引导；交付链正常。
已发布源码在 iPhone 13 Pro（iOS 26.6.2）上逐字复现了现场提示。

修复内容：手机紧凑设置说明（可收起）、桌面动态范围说明与「N 张未共享」、设备行最近同步与 build，以及经用户确认的**按设备信任**（只授予该设备完整输出、便签与直接消息，不改全局范围、不加 shell 输入或自动化权限）。
同时修复了只在真机上暴露的问题：离线 45 秒不可见、`URLError` 被抹掉、本地网络被拒时提示错误、授权后误报「目标已变化」、小屏首屏被说明占满。

最终代码真机全套 13/13 + 清理用例 Z1 通过（真实 Wi‑Fi HTTPS、真实 Claude，消息与响应丢失各只送达一次，由 Claude 会话记录与主机日志独立核对）。
门禁：Rust workspace 1,541、覆盖率步骤、UI 850、Swift Core 85、模拟器 AppModel 15/15 与 UI 5/5 + 1/1 通过；fmt、clippy、release / EDR 扫描通过。
`cargo audit` FAIL：HEAD 既有的 `zip 4.6.1`（经 updater 引入）。
BLOCKED：锁屏期间 Keychain 不可读（设备未要求密码）。
UNRUN：Mac 睡眠、VPN、Mac 端重启 Deck、旧主机 + 新客户端。
提交 `9a92480`；随后 `be9ae97` 修复了 MCP 拒绝被误解释为排队限制的问题（P2），并去掉了更新插件中仅 Windows 使用的 zip 依赖，`cargo audit` 恢复通过。
经用户授权发布 Nightly 0.8.5（`8a8d6eb`，run 39023645510，一次通过）；发布后核对了 DMG 公证、Gatekeeper 与哈希。
iOS 1.0 (4) 已上传，App Store Connect 处理状态未在本会话核实；见 `publication.json`。

## 上一轮 Phone Connector 现有卡片访问

基线 `cf18861`；修复与隔离证据见
[现有卡片输出与直接交互](evidence/phone-connector-access-20261017/README.md)。
新增 Mac 明确确认的「全部卡片输出和便签」范围：允许空启动命令/手动 Agent 的输出、便签和当前 Codex/Claude 直接消息；普通 shell 只开放输出与便签。
旧授权不升级，队列仍需保存的启动配置，MCP/目标代次门禁保留；撤权检查延伸到 GET 返回、每次实际输入和便签落盘边界。

Rust workspace 1,537 项、UI 849 项、Swift Core 80 项与 registry 30 项通过；Rust 13 项原有 ignore、Swift 1 项 opt-in 跳过保留。
fmt/clippy、JS 语法/模块和扩展目录检查通过。
iPhone SE（iOS 27.0、深色）隔离 UI 5/5、退出到 shell 1/1 通过：真实 Codex 卡片保存命令为空，手机消息发送后读取拼接回复标记；shell 便签实际保存/读回/删除，shell 发送始终禁用。
首轮测试操作失败及修复如实记录，所有实验资源按 registry 清理。真机 Wi-Fi/VPN、锁屏/后台、Mac 睡眠仍为 UNRUN。
随后经用户明确授权，提交 `e41bf49`，发布 [0.8.4 Nightly](https://github.com/c9r-io/deck/releases/tag/nightly-v0.8.4-20261016-e41bf49) 与 TestFlight **1.0 (3)**（「个人设备」正在测试）。
发布前 14 个 WKWebView 场景最终通过；完整 CI 和 Apple 验证通过，公开产物哈希、Nightly 签名及 0.8.4 更新清单已核验。详见本轮证据的 `publication.json`；未替换本机安装。

## 上一轮第一方插件职责治理

基线 `3261805`；实现与隔离验收见
[第一方插件职责治理](evidence/first-party-governance-20261016/README.md)。
Slack/Bergamot 的 Settings controller 收归贡献模块；Phone Connector、MCP（含 Tunnel 与授权对话框）
和语音偏好纳入自动目录。Connector/MCP 使用独立 access 注册，语音仅注册 UI；核心授权和设置事务保持。
原有 400 个界面元素及属性、搜索顺序完整保留，仅增加 3 个隐藏挂载位置。

UI 847 项、Rust workspace 1,527 项、Python 190 项通过，Rust 13 项原有 ignore 保留；
真实 WKWebView 综合 94/94、Settings 12/12（新增 5 项 MCP）、voice 18/18，
以及 Connector、channel、approval、empty-start、signal-finish、Hooks、真实 Sandbox A Slack、
翻译四场景和 Pi npm/布局验收均通过。完整本地门禁及首次失败/修复见报告。
所有原生测试采用隔离 HOME、数据、carrier 与 tmux socket，按 registry 清理；
正式 Deck 进程与配置哈希未变。macOS 14、远端 CI、真实 Slack 消息投递与麦克风录音仍为 UNRUN，未发布。

## 上一轮第一方插件自动注册

基线 `fb12d0a`；三步实现与隔离验收见
[第一方插件自动注册](evidence/first-party-autoreg-20261016/README.md)。
构建时发现第一方声明并生成双端注册表，运行时挂载 Settings contribution；
Claude Code、Codex、Pi、Slack、Bergamot 已接入。既有控制、搜索顺序、持久化和授权边界保持。

UI 845 项、Rust workspace 1,527 项、Python 189 项通过，Rust 13 项原有 ignore 保留；
覆盖率、fmt/clippy、静态门禁、CE 本地校准、release 构建和本地兼容载体打包通过。
真实 Sandbox A Slack 连接/恢复、翻译下载/安装/重启、Pi npm 与布局、Settings 原生验收通过；
综合真实 WKWebView 94/94 通过。
Hooks 原生 8/8 通过；额外全文件往返断言揭示既有空事件数组清理语义，失败与基线比对保留。
完整细分结果、首轮失败及复验在报告中；macOS 14、远端 CI、真实 Slack 消息投递仍为 UNRUN。
本机正式安装未替换，测试资源隔离并按实验 registry 清理，未发布。

## 上一轮终端输入附着边界治理

基线 `817df9b`；修复与证据见
[终端输入附着边界治理](evidence/terminal-input-governance-20261015/README.md)。
文件拖入/图片粘贴、补全/ghost 和菜单清屏统一进入按附着代次绑定的输入队列；
文件读取前预留顺序，换附着取消旧输入，完成时不抢回焦点。
后端拒绝缺失/null/过期代次，不增加授权或 readiness 来源。

UI 837 项与 Rust workspace 1,521 项通过，13 项保留原有忽略；fmt/clippy、
语法/模块和 debug 构建来源检查通过。首轮 Rust 有一项未改动的生命周期超时测试
超限，原断言下定向 8 项及完整 workspace 重跑均通过，失败记录保留。
反向变异准确触发四组旧附着回归失败。

**原生验收 BLOCKED**：本机锁屏导致窗口不可读取、计时延迟，首轮 smoke 不计通过。
需解锁后重跑新的隔离 `terminal-input` smoke（15 个检查点），不能据单元测试扩大
WKWebView 认证。该轮应用和测试会话已精确清理，最终 inventory 退出 0；未发布。

## 上一轮 Slack 频道配置引导

基线 `c53d3e7`；实现与隔离验收见
[Slack 频道监控配置治理](evidence/slack-channel-setup-20261015/README.md)。
入口提供既有 App 的配置合并、Manifest/OAuth 页面跳转、token 保存后验证并启用，
失败保留草稿，取消阻止尚未开始的启用写入。连接建立后有限次数自动刷新，成功提示可见。

完整 UI gate 826 项、Rust workspace 1,521 项通过，13 项保留原有忽略；fmt/clippy、
模块与语法检查、隔离构建来源检查通过。真实 WKWebView channel smoke 3/3 通过。
用户指定的 Slack 沙盒已验证真实凭据、既有 App 页面、配置合并、Socket 连接与自动状态反馈。
该 App 原有权限齐全，未执行新增权限授权；没有发送真实频道消息，消息投递不记作 PASS。
四次 smoke 启动均已清理，最终 inventory 退出 0；正式 Deck 未操作，未发布。

## 上一轮键盘导航与注意力返回治理

基线 `0743224f`；实现与证据见
[键盘导航与注意力返回治理](evidence/navigation-governance-20261014/README.md)。
卡片、侧栏和项目提供原生键盘打开入口，重绘保留焦点；从注意力定位后可直接返回，
继续打开同一卡片仍保留筛选、滚动与原行。大字号下标题与徽标分别占行，避免入口被挤至零宽。
定位不启动会话，普通用户打开动作继续使用原有语义；没有新增 schema 或授权来源。

完整 UI gate 816 项、五组原生合约 55 项、语法/模块及 fmt/clippy 通过。
完整 Rust workspace 1520 项通过、13 项保留原有忽略，0 失败。
隔离真实 WKWebView 51/51 检查点通过，其中新增导航检查包含 18 个条件；
中英文 × 100%/160% 的 4 组入口布局及既有 12 组注意力布局通过。
真实键盘验证项目 Enter/Space 切换和侧栏 Enter 打开。三次 smoke 按精确 socket 清理，
最终 inventory 退出 0。正式安装、真实 Agent 集成与远端 CI 未执行；投递恢复和注意力密度仍待后续治理。

## 上一轮 Settings 状态反馈治理

基线 `9a3b855`；本轮实现与证据见
[Settings 状态反馈治理](evidence/settings-feedback-governance-20261014/README.md)。
Hooks 读取失败、超时和缺失/非法字段显示未知并禁用，读取重试不写配置；
普通设置与 Hooks 保存/应用结果逐项显示，失败跨设置窗口关闭保留，侧栏提供数量与定位入口。
反馈仅在当前应用进程内保留，未增加 schema 或授权来源。

完整 UI gate 811 项、五组原生合约 55 项通过，语法/模块检查与 debug 构建来源复核通过。
真实隔离 WKWebView 注入 native 保存失败，验证回滚、跨重开提示和定位；
中英文、100%/160% 字号、8 个设置区共 32 组布局及 7/7 smoke 检查点通过。
测试资源已按精确 socket 清理，最终 inventory 退出 0。真实 Hooks 写入、正式安装、完整 Rust
workspace、fmt/clippy 与远端 CI 在该轮未执行；键盘入口的后续证据见本轮记录。

## 上一轮 Settings 提交一致性治理

基线 `ce872c1`；本轮工作树的修复、回归和本地验证见
[Settings 提交一致性证据](evidence/settings-write-governance-20261014/README.md)。
每次写入只应用该操作的字段修改；失败后从已落盘状态和剩余预览重建界面。
编辑器、语言和语音经过同一提交路径；原生语言更新在落盘后执行，生效失败不回滚已保存选择。

新增 18 项 DOM 回归；完整 UI gate 804 项通过，原生 IPC/EDR/隐私/Signal/所有权合约
55 项通过；debug 构建及 sidecar/frontend staging 来源检查通过。首次保存、规则撤销、
原生 channel grant 保留的既有测试继续通过。实际保存和原生菜单调用在新用例中均由 mock 驱动；
本轮未执行真实 WKWebView、用户配置、Slack/Agent 客户端、签名安装、远端 CI 或完整 Rust workspace。
以上范围不能扩大上一轮原生认证；修改 IPC、schema 或授权语义时须重验相应原生场景。

## 上一轮 Board 与授权边界治理

基线 `e1bd78e`；该轮已提交为 `ce872c1`，改动与命令结果见
[Board / authority 治理证据](evidence/board-authority-governance-20261013/README.md)。
这是本地验证，不是该工作树的远端 CI 或原生重新认证。

| 项目 | 本轮范围 | 原生范围与触发条件 |
| --- | --- | --- |
| 冻结 channel head | documents 的最小类型投影供实际 admission 消费；明确 false 与缺省值分开；独立字段篡改、扩展值、恢复/重放继续回归 | 不改变传输、readiness、grant 语义或锁顺序，沿用下表原范围；未来改变任一语义时重跑对应原生场景 |
| 撤销时点 | tick candidate 的事务回滚/一次撤销；隔离真实文件与生产 send pipeline 在 prepare/final_probe 后撤销，确认零 fire 与持久撤销 | fake probe 不是真实 pane 输入认证；pre-start/pre-fire 栅栏均保留 |
| Board 卡片展示 | `board-card.js` 负责 DOM 与手势路由；Board 保留动作 owner；完整 UI gate 与新增 DOM 行为测试 | DOM/CSS 与原主体一致；本轮不重跑 WebKit 尺寸/focus/click smoke，未扩大旧 PASS |

本轮最终门禁、构建来源、退出码和文件哈希统一记录在上述证据目录，历史结果不转写成新代码的 PASS。
本地 UI 786 项、Rust workspace 1,520 项通过（原有 13 ignored）；fmt、clippy、模块检查与
release 二进制/来源检查通过。远端 CI、Rust llvm-cov、真实 GUI/客户端和签名安装本轮未执行。

## 上一轮 hooks 与验证同步

基线 `84b7783`（0.8.1）；该轮结果已在 `e1bd78e` 提交，文件哈希、命令、结果和失败经过见
[治理证据](evidence/technical-debt-20261013/README.md)。
基线的 [test](https://github.com/c9r-io/deck/actions/runs/38284431702) 与
[nightly](https://github.com/c9r-io/deck/actions/runs/38284433607) 已通过，
它们不覆盖之后的 `e1bd78e` 治理修改或当前工作树；本轮未核验更新的远端 CI。

| 项目 | 已有证据和范围 | 仍未证明 | 下一次验证触发条件 |
| --- | --- | --- | --- |
| tmux 测试启动与重建同步 | 本轮检查每条预期成功命令的退出码；重建前观察旧 server 的内核退出事件；完整合约测试与重复拓扑测试，见本轮证据 | 历史 CI 的 shell prompt 超时未在本机复现，仍为**待观测**；不声称彻底消除 flaky test | 下一次正常 push 的 CI；若再失败，保留本轮新增的具体命令、退出码与 stderr，按失败阶段定位，不靠重跑变绿 |
| Pi transport 测试同步 | 心跳场景通过 fixture 的写入完成标记同步；保留普通 observer 的 nonce 重试行为；完整 transport 测试，见本轮证据 | 真实 Pi 的 native 矩阵没有在本轮重跑；定时失效测试仍有其独立时间预算 | 改 transport、observer 或 fixture 协议时重跑相关单测；改变进程证明语义时重跑 native 矩阵 |
| hooks 配置模块 | 配置、安装、迁移与对应测试搬入 `agent_status/hooks.rs`；生产主体逐段比对仅空白变化；原 IPC 路径通过父模块导出 | 没有在本轮启用用户的真实 hooks，也没有重新执行签名 release 的设置开关 | 修改编辑器、路径准入、迁移语义时重跑配置测试及相应真实窗口场景；仅移动代码不能扩大原生认证范围 |
| smoke 剪贴板读取 | 固定 `/usr/bin/pbpaste` 与 `LC_ALL=en_US.UTF-8`，避免缺失 locale 或继承 C locale；命令构造、smoke 禁用保护与 EDR 检查 | 本轮没有读取或改写用户剪贴板，也没有重跑真实窗口的非 ASCII 拷贝场景 | 下次 clipboard WK smoke 在缺失 locale / C locale 两种启动环境下检查字节；失败时区分生产 writer 与 smoke reader |

## 近期功能的原生验证范围

| 能力 | 当前可沿用的证据 | 不应写成已完成的部分 | 重验触发条件 |
| --- | --- | --- | --- |
| Pi run 边界与 native session 观察 | [Pi 收尾](evidence/pi-closeout-20261012/README.md)：P1a、P2 的 native 矩阵与窗口证据绑定 `8bb2b7f`；后续产品改动的适用范围在原文逐项说明 | needs-input、re-entry 为 **BLOCKED**；通知实际弹出、其它 Pi 版本、签名发布包等仍按原表 **UNRUN**。已接受残余风险不等于消除 | Pi 原生协议／运行时改变，或观察者／身份绑定逻辑变化；上游提供可靠输入信号或代码身份证明时再评估缺口 |
| Codex shared-daemon Signal | [归属认证](codex-signal-certification-20261002.md)：共享 daemon 的身份归属限制；embedded topology 与 shared daemon 分开判断 | 无可验证的每客户端 pane 绑定时仍不可授信。安全拒绝不等于注意力覆盖完整 | 官方来源提供新 provenance 原语，或改变 hook 进程拓扑时重新认证；不以 cwd、时间或会话 id 补造归属 |
| Claude 重启后 re-entry | [验收矩阵](evidence/closeout-20261011/reentry-acceptance.md)与[收尾状态](evidence/closeout-20261011/README.md#verdict)：debug，以及迁移开启观察的 Developer-ID release carrier，在 relocated home 的代表性事务 | `/Applications/deck.app`、release 设置开关、真实窗口中的未知结果、macOS 26 真实客户端等仍受原表限制；私有载体未公证 | 改身份验证、启动／退出 receipt、事务或安装位置时重验；需要实际安装验证时使用满足隔离条件的环境 |
| Codex（`--no-daemon`）重启后 re-entry | [证据](evidence/codex-reentry-20261018/README.md)：隔离 Codex home + 私有 tmux，测试控制器跑生产事务，真实 `codex-cli 0.163.0`；Policy A 与 B 各一次全检通过（`a2`、`b1`），`a1` 的 4 项失败是控制器自身缺陷并已修正；事后 `/status` 读到退出时的会话与预期权限；假客户端覆盖多线程退出、未打印、打印未报告 id 的跳过路径 | 真实应用窗口／设置开关写 `~/.codex/hooks.json`／Codex 信任提示未运行；共享 daemon 客户端按设计拒绝；zsh 以外的 shell、真实 `/new`、goal／队列自动回合、macOS 26、发布载体未验证 | Codex hook 事件集合、退出摘要文案、resume 权限恢复语义或签名身份变化时重验；上游提供 SessionEnd `active` 标记或 resume 回执后删除文本增量这一支 |
| Slack 与频道首步 | [首步本地验收](channel-first-send-verification.md)、[后续收尾](evidence/closeout-20261011/README.md#b-the-slack-integration-in-the-real-window)：合成事件／隔离 fixture 与后续真实 Slack debug 窗口证据分别成立 | release 窗口、channel-scope 故障和跨退出 pending 事件等不因通用 gate 通过而关闭 | 改 grant、冻结计划、撤销、重放或最终发送时重验相应单元／故障用例；改传输时补真实服务验证 |
| 构建产物来源与实验清理 | [收尾 A 与 registry](evidence/closeout-20261011/README.md)：已修复共享产物污染、打包后复核与资源登记；本轮继续执行相应回归 | 历史误杀的会话不可恢复；之后的通过不抵消事故。旧 runner 的未重跑记录仍保留 | 新增打包入口或实验 runner 时，先接入 provenance／registry，再运行；不得按默认 socket 名清理 |

## 治理停止边界

前两轮分别拆 hooks 配置、收窄 channel head 证据表达和 Board 卡片展示职责。
此前两轮治理 Settings 写入、预览、失败回滚及状态反馈；本轮治理键盘入口和注意力返回连续性。
Board 的 polling/lifecycle、其它来源的 JSON 授权消费、名称字符/字节兼容性和构建系统不在本轮范围。
唯一 writer、副作用入口、恢复来源、撤销条件与锁顺序保持原状；以职责和依赖边界为验收依据，
不以文件行数作为完成条件。第一方静态表继续保持窄接口，不扩成通用插件系统。

更新此页时保留被测提交和环境；新代码若改变旧结论的前提，先把对应行标为待重验，
不要仅更新日期。证据中涉及的凭据、用户文件内容、内部地址和 native session id 不收录到这里。
