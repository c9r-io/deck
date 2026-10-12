# Pi v0.8.0 收尾证据，2026-10-12

Pi（Pi coding agent）第一方集成从 origin `1bdcf57` 之后的全部工作，以及每一项现在的状态。状态就是“状态表”里一项一项的那些；其中有 BLOCKED 和 UNRUN，所以这里没有任何一句话说“Pi 集成全部完成”。

本文件由执行 Agent 撰写，是提交给负责人复核的材料，不是验收结论。表里的 PASS 是“执行者自评为通过，并且负责人的独立验收也通过”；负责人的验收记录不在版本控制内（见“证据目录索引”）。

这是一份干净检出就能读懂的摘要。原始日志、逐次运行记录和运行脚本在执行者本机被忽略的 `reports/` 里，没有任何检出带着它，下面也没有一处需要打开它。这里没有记录凭据、账号、消息内容或 Pi 的 native session id。

状态用词：**PASS** 跑过并且成立；**FAIL** 跑过并且不成立；**BLOCKED** 因为代码之外的、说得出名字的原因无法进行；**UNRUN** 没有运行。“沿用”指更早一轮的证据，本轮没有重新执行。

## 1. 状态表

| 项 | 状态 | 原因 | 证据 |
| --- | --- | --- | --- |
| P1a：run 边界的 Signal（Pi 开始工作、一次交互结束） | PASS（沿用 `8bb2b7f` 上的证据） | 真实 Pi 两种安装方式的 native 矩阵 20 个场景 282 项检查通过；真实窗口三次运行通过；一行是否算作绑定进程自己写的，由四件事一起证明（内核给出的对端 pid、自上次查看以来没有 fork、连接晚于那次查看、一次性的 nonce），真实 Pi 的工具子进程只带来一次重试 | `docs/evidence/pi-followup-02-20261012/`（`native-result.json`、`native-scenarios.json`、`wk-*.json`、`shots/`） |
| P2：native session 观察（Pi 正在哪个 session 里） | PASS（沿用 `8bb2b7f` 上的证据） | session 的帧走同一套证明；Deck 不保存的 id 明说“不支持”，不截断、不静默丢弃；id 只在内存里，不显示、不记日志、不落盘 | 同上 |
| 完整 P1：needs-input（Pi 在等输入） | **BLOCKED** | 上游没有可靠信号：Pi 1.1.x 的界面提示通知不带身份、可能互相超越，内置对话框不发通知。Deck 不伪造这个状态：Pi 的卡片从不显示“请求输入”并在卡片上说明，对已识别的 Pi 一律不自动发送 | `app/src-tauri/src/agent_pi.rs` 头部；`docs/evidence/pi-investigation-20261012/` |
| P3：re-entry（重启后自动重新打开 Pi） | **BLOCKED** | 两种安装方式都无法让 Deck 证明运行的是谁的代码：npm 安装的运行映像是 Node；官方 standalone 是 ad-hoc 签名。Pi 没有登记 re-entry 能力，重启复核里点名 Pi 并给出它自己的理由。拒绝路径的测试通过不是 P3 通过 | `agent_pi.rs` 头部；`docs/first-party-extensions.md` “Optional re-entry capability” |
| 残余风险 1：建立连接期间别的线程 fork 出的子进程 | 已接受（负责人裁决，依据是测得的事实） | 两种运行时都在 JS 线程的一次原生调用里建好连接，常规子进程什么描述符都不继承；但 Bun 的 `Bun.openInEditor` 在自己的线程上 fork，1000 个这样的子进程里有 9 个（负责人复现 8 个）生来持有半建成的连接。这样的子进程没有 nonce：代价上限是一次重试或一次重新绑定，它写的任何一行都不被采信 | `app/src-tauri/src/agent_pi/transport.rs` 头部第 118–128 行；`docs/evidence/pi-followup-03-20261012/` |
| 其余残余风险（进程主动交出连接或 nonce、所依赖的内核语义没有文档保证、拥塞时覆盖降级等七条） | 已列出，未消除 | 逐条的范围与后果见所列章节 | `docs/evidence/pi-followup-02-20261012/README.md` §14 |
| 门禁的本机形式（最终代码树） | PASS，另有 2 步 UNRUN | 见第 4 节 | `gates.json` |
| native 矩阵与真实窗口运行（最终代码树） | **UNRUN** | 本轮只要求在最终代码树上跑门禁。`8bb2b7f` 之后产品源码只改了注释（`transport.rs`、`agent_status.rs` 的非注释行改动为 0）、加了一个测试；沿用的是 `8bb2b7f` 上的结果，它绑定的是当时的源码哈希 | 第 4 节 |
| 残余风险 1 的子进程对 Deck 实际 transport 的效果 | **UNRUN** | “代价上限”是执行者与负责人按 `commit` 与 `observer.ts` 的代码一致推断的，没有实测 | `transport.rs` 头部；`pi-followup-03-20261012/README.md` |
| 有意留空：一次 Pi 交互结束时 away notification 实际弹出 | **UNRUN** | ad-hoc 签名的载体不能发布通知 | `pi-followup-02-20261012/README.md` §15 |
| 有意留空：Pi 旁边同时有已登录的 Claude Code / Codex | **UNRUN** | 各轮都不使用任何账号 | 同上 |
| 有意留空：签名的发布包 | **UNRUN** | 没有授权 release；真实窗口用的是本地 debug 载体（ad-hoc 签名），没有任何东西在 `/Applications/deck.app` 运行 | 同上 |
| 有意留空：其它 macOS 版本（含门禁里的 macOS 14 启动检查） | **UNRUN** | 开发机与测试机都是 macOS 27.0.1 | 同上；`gates.json` |
| 有意留空：Pi 1.1.0 以外的版本 | **UNRUN** | Deck 接受 1.1.x，实际跑过的只有 1.1.0 | 同上 |

## 2. 提交地图

origin `1bdcf57` 之后、到本提交为止的 33 个本地提交，按轮分组。全部在本地 `main`，没有 push、merge 或 tag。

**首轮：设计、调查与实现**（负责人独立验收：未通过，见 `pi-implementation-20261012/README.md` 文首的更正）

| 提交 | 内容 |
| --- | --- |
| `ea6d3fe` | P1a：随应用打包的 observer、Pi 设置里的一条 entry、有界的传输，以及对 Pi 不自动发送。 |
| `812b786` | P2：native session 观察作为单独的开关；Claude 的 receipt 改由显式契约声明。 |
| `3c92663` | 重启复核点名已验证的 Pi，并以它自己的理由拒绝。 |
| `50622f3` | 设计文档、调查与首轮实现的证据（设计文档后来移出版本控制）。 |
| `7e4f8d7` | 在最终树上重跑两次真实窗口运行。 |

**Follow-up 01：F01–F06**

| 提交 | 内容 |
| --- | --- |
| `b59e88d` | 负责人对 `7e4f8d7` 的验收与 Follow-up 01 文档，原样提交（后来移出版本控制）。 |
| `34082fd` | F01、F02、F03、F06：每个 lease 一个执行实例；不支持的 id 明说；能力逐项收回；首行有界。 |
| `599095b` | 测试：native 运行标明自己的构建，覆盖 exec、每个开关单独、Deck 不保存的 id。 |
| `0d826fb` | 测试：真实窗口里 Pi 的行，以及真实后端的真实对话框。 |
| `a18aa8a` | 更正首轮实现证据里的两项判定（F05）。 |
| `4427630` | F03：绑定时能力少于运行时已加载的 observer，在被接受之前得知。 |
| `ef37976` | 测试：每个开关在每种运行时上（空闲与运行中）、真正在途的帧、真实的 Needs attention 视图。 |
| `eb16c71` | 测试：真实窗口运行在签名之前比对载体与构建产物（被测提交）。 |
| `546eb45` | Follow-up 01 的证据。 |

**Follow-up 02：R01–R03（发送者证明，wire v2）**

| 提交 | 内容 |
| --- | --- |
| `2153972` | 负责人对 Follow-up 01 的验收与 Follow-up 02 文档，原样提交（后来移出版本控制）。 |
| `125590e` | R03：native 驱动读到后端的第一条事件；破坏 framing 的一行立即让运行失败。 |
| `c5aa34e` | R01、R02：一行只有带着内核的对端 pid、Deck 的 nonce、其间没有 fork，才算 observer 自己的；每条连接一条消息。 |
| `9705585` | 测试：native 运行对不保持连接的 lease 说话，让 Pi 跑自己的工具，并让工具与子进程尝试冒名。 |
| `6712813` | 没有绑定时准入 worker 不设定时器；修正真实进程测试里一处由调度决定的先后。 |
| `ad2b0fd` | 测试：exec 之前缓冲的内容也不能为新映像指名 session。 |
| `f37cdd2` | R01 补充修复：一行只在“晚于 Deck 上次查看该进程才建立”的连接上算数。 |
| `8bb2b7f` | EDR 契约与三处注释改为现状；一条测试不再依赖等待时长（被测提交）。 |
| `8a5a64b` | Follow-up 02 的证据。 |

**Follow-up 03：文档移出版本控制、头部文字、残余风险 1 的测量**

| 提交 | 内容 |
| --- | --- |
| `0a1d525` | 设计文档与负责人的验收记录移出版本控制，文件留在磁盘上。 |
| `dfec67c` | transport 头部说明迟到的 attest 仍记录什么、一次查看按什么次序进行（只改注释）。 |

**Follow-up 04：文字与证据**

| 提交 | 内容 |
| --- | --- |
| `736286a` | transport 头部把“建立连接是一步”从假设改为测得的事实；`agent_status.rs` 的过时注释（只改注释）。 |
| `7f2dd68` | 首轮验收记录移出版本控制；四个 README 里的七处引用改为纯文字。 |
| `823d098` | Follow-up 03 测量的证据。 |

**Follow-up 05：收尾（本轮）**

| 提交 | 内容 |
| --- | --- |
| `97ef076` | `.gitignore` 用两条模式规则忽略设计文档与负责人的验收记录，不再逐目录加行。 |
| `85f3a03` | “挂起期间 JS 线程为什么偶尔停住”的补测摘要与程序并入 Follow-up 03 的证据。 |
| `d587b38` | `CLAUDE.md` 的模块表加上 Pi 适配器一行；`docs/first-party-extensions.md` 里 Pi 的调用路径行补上路径与 hold 的名字。 |
| `bb0731f` | 一个测试，钉住 observer 与 transport 两端约定的两个时间。 |
| 本提交 | 本目录：收尾证据与最终代码树上的门禁记录。 |

## 3. 证据目录索引

`docs/evidence/` 下与 Pi 有关的、在版本控制内的目录：

| 目录 | 是什么 |
| --- | --- |
| `pi-investigation-20261012/` | 设计阶段的调查：Pi 1.1.0 的官方 artifact、原生事件与 settlement、session、扩展加载；没有改产品代码。 |
| `pi-implementation-20261012/` | 首轮实现（`7e4f8d7`）的证据。负责人验收未通过；文首有 Follow-up 01 加上的更正。 |
| `pi-followup-01-20261012/` | Follow-up 01（F01–F06）在 `eb16c71` 上的证据。 |
| `pi-followup-02-20261012/` | Follow-up 02（R01–R03）在 `8bb2b7f` 上的证据：内核原语的实验、native 矩阵、三次真实窗口运行、门禁。P1a 与 P2 的结论依据的是这一轮。 |
| `pi-followup-03-20261012/` | 残余风险 1 的有界测量（不运行 Deck），以及 `stall/` 里对测量方法副作用的补测。 |
| `pi-closeout-20261012/` | 本目录：本文件与 `gates.json`。 |

负责人的文档是负责人本地文档，不在版本控制内，由 `.gitignore` 的两条规则忽略：设计目录 `docs/design/pi-v0.8.0/`（设计、裁决与各轮 prompt），以及六份验收记录 `docs/evidence/pi-acceptance-20261012/`、`docs/evidence/pi-followup-01-acceptance-20261012/`、`docs/evidence/pi-followup-02-acceptance-20261012/`、`docs/evidence/pi-followup-03-acceptance-20261012/`、`docs/evidence/pi-followup-04-acceptance-20261012/`、`docs/evidence/pi-followup-05-acceptance-20261012/`（最后一份在本轮之后才写）。

## 4. 门禁

按 `.github/workflows/gate.yml` 的步骤顺序，在本机跑了它能跑的每一步，记录在本目录的 `gates.json`（格式与 `pi-followup-02-20261012/gates.json` 相同）。

- 被测提交：`bb0731f32d4bbd94e84e659b01ddc4b7bd52eda1`，即本提交的父提交。本提交只增加本目录的两个文件，不改任何代码、测试或脚本。
- 工作区：开始时 0 个改动项，结束时 0 个；结束时 HEAD 仍是 `bb0731f32d4bbd94e84e659b01ddc4b7bd52eda1`。
- 时间：2026-10-12T15:09:43Z 至 2026-10-12T15:23:31Z（UTC）。环境：macOS 27.0.1（arm64）开发机，隔离的 HOME，保留 `RUSTUP_HOME` 与 `CARGO_HOME`，`TMUX`、`TMUX_PANE`、`DECK_STATUS_SOCK` 不带入。
- 结果：运行的 30 步全部 exit 0；2 步 UNRUN。

| job | 步骤 | exit | 计数与输出 |
| --- | --- | --- | --- |
| static | `tmux-sha` | 0 |  |
| static | `bergamot-patches` | 0 |  |
| static | `fmt` | 0 |  |
| static | `js-syntax` | 0 |  |
| static | `ui-tests` | 0 | 781 通过 / 0 失败 / 0 跳过（共 781） |
| static | `speech-bridge` | 0 |  |
| static | `notification-bridge` | 0 | notification bridge: helpers and no-bundle guard ok |
| static | `smoke-guard` | 0 | 变异 5 个全部被发现 |
| static | `check-mjs` | 0 |  |
| static | `python-unittest` | 0 | 181 项，OK |
| static | `check-workflows` | 0 |  |
| static | `audit-app` | 0 | 无漏洞；8 条已允许的警告 |
| static | `tunnel-fmt` | 0 |  |
| static | `tunnel-clippy` | 0 |  |
| static | `tunnel-test` | 0 | 28 通过 / 0 失败 / 3 ignored |
| static | `audit-tunnel` | 0 | 无漏洞 |
| rust | `clippy` | 0 |  |
| rust | `ce-probe-env` | 0 |  |
| rust | `llvm-cov` | 0 | 1518 通过 / 0 失败 / 13 ignored；覆盖率：行 85.78%、函数 83.75%、区域 86.60% |
| rust | `provenance-coverage` | 0 | sidecar provenance ok: 4 files in <repo>/app/src-tauri/binaries are the debug/coverage build of this checkout |
| rust | `ce-runner-build` | 0 |  |
| rust | `ce-parity` | 0 |  |
| rust | `ce-verdict` | 0 | ce-verdict gate full_local_parity:ci: pass |
| release | `release-build` | 0 |  |
| release | `release-translation` | 0 | static Bergamot markers present; no dynamic translation dependency in app/src-tauri/target/release/deck-app |
| release | `release-edr` | 0 | EDR binary vocabulary clean: app/src-tauri/target/release/deck-app |
| release | `package-compat` | 0 |  |
| after | `debug-build` | 0 |  |
| after | `test-build` | 0 |  |
| after | `provenance-debug` | 0 | sidecar provenance ok: 4 files in <repo>/app/src-tauri/binaries are the debug build of this checkout |
| macos14-launch | `test-compat-candidate` | **UNRUN** | 工作流在 macOS 14 的 runner 上经 LaunchServices 启动打包好的候选包；本机是 macOS 27.0.1，手头没有 macOS 14 的机器。候选包本身已在本机打包（`package-compat`）。 |
| gate | `gate-results` | **UNRUN** | 工作流的汇总 job 从工作流运行本身读取其它 job 的结果；本机没有这样一次运行，这份记录就是替代它的东西。 |

`llvm-cov` 一步里每个测试可执行文件的结果：

| 测试可执行文件 | 通过 | 失败 | ignored |
| --- | --- | --- | --- |
| `deck_app (src/main.rs)` | 1244 | 0 | 13 |
| `edr_quiet (tests/edr_quiet.rs)` | 20 | 0 | 0 |
| `external_admission (tests/external_admission.rs)` | 8 | 0 | 0 |
| `ipc_contract (tests/ipc_contract.rs)` | 6 | 0 | 0 |
| `log_privacy (tests/log_privacy.rs)` | 7 | 0 | 0 |
| `session_architecture (tests/session_architecture.rs)` | 13 | 0 | 0 |
| `shared_output (tests/shared_output.rs)` | 15 | 0 | 0 |
| `signal_census (tests/signal_census.rs)` | 9 | 0 | 0 |
| `tmux_contract (tests/tmux_contract.rs)` | 111 | 0 | 0 |
| `channel_fixture (examples/channel_fixture.rs)` | 0 | 0 | 0 |
| `fake_reentry_agent (examples/fake_reentry_agent.rs)` | 0 | 0 | 0 |
| `local_translation_quality (examples/local_translation_quality.rs)` | 0 | 0 | 0 |
| `poll_bench (examples/poll_bench.rs)` | 0 | 0 | 0 |
| `pty_smoke (examples/pty_smoke.rs)` | 0 | 0 | 0 |
| `signal_fixture (examples/signal_fixture.rs)` | 0 | 0 | 0 |
| `deck_lifecycle_helper (src/main.rs)` | 27 | 0 | 0 |
| `cli (tests/cli.rs)` | 8 | 0 | 0 |
| `deck_mcp (src/main.rs)` | 2 | 0 | 0 |
| `stdio (tests/stdio.rs)` | 5 | 0 | 0 |
| `deck_mcp_runner (src/main.rs)` | 19 | 0 | 0 |
| `runner (tests/runner.rs)` | 10 | 0 | 0 |
| `deck_status_helper (src/main.rs)` | 8 | 0 | 0 |
| `cli (tests/cli.rs)` | 6 | 0 | 0 |

job 名沿用上一份记录：`static` 对应工作流的 `checks`，`rust`、`release` 同名，`after` 不是工作流的一部分——release 构建会替换共用的 sidecar 与前端，所以结束前把本检出的 debug 构建与测试可执行文件重新建好并核对其来源。工作流里的检出、工具链安装、缓存、产物上传、构建计时，以及把 runner 账号的登录 shell 设为 zsh 的那一步，是设施步骤，没有对应项（本机账号的登录 shell 本来就是 zsh）。

这次 release 构建的可执行文件 SHA-256 是 `65a068a9b267a078217e4f931f99dc724a1452ade9a44c36aeb9abaf4742766b`，打包出的兼容性候选包是 `3657bcfd2aca4470cd013347dc4c343af672a2e6b1f67302d44619fcce39312f`（它就是 macOS 14 那一步本该启动的包）。

没有在最终代码树上重跑的：真实 Pi 的 native 矩阵与真实窗口运行（沿用 `8bb2b7f` 上的结果，见状态表）；`llvm-cov` 一步里标为 ignored 的测试包括需要真实 Pi 的那一个。

## 5. 发布说明草稿

供发布时放进 `CHANGELOG.md`（本轮没有改 `CHANGELOG.md`）。口吻与详略照 0.8.0 一节；事实依据 `agent_pi.rs` 头部、`agent_pi/config.rs` 头部和界面上的英文文案（`app/ui/js/i18n/en.js`）。

```markdown
- New, off by default: Deck can show when Pi (the Pi coding agent) starts
  working in a card and when an interaction ends. Turn on "Pi work status"
  in Settings → Agents & notifications; it adds one entry to the
  `extensions` list in Pi's settings file (`~/.pi/agent/settings.json`)
  that names a read-only extension inside the Deck app. Your other settings
  and extensions are kept, no program is copied into your home directory,
  and turning it off removes exactly that entry. A Pi that is already
  running follows when it next starts or after `/reload`; turning the
  setting off takes effect at once.
- What Pi status does not cover: Deck cannot reliably observe when Pi is
  waiting for your input, so a Pi card never shows "input requested", and
  the card says so ("Pi connected · limited status"). An ended interaction
  does not mean the task is finished or succeeded. When Deck has no status
  from the Pi in a terminal the card says "Pi status unavailable", and for
  a Pi release outside the supported range, "Pi version not supported".
- Nothing is sent to Pi automatically. Queued steps and automations that
  would go to a terminal running Pi are held ("automatic sending to Pi is
  paused"), whether or not Pi work status is on; use Send now after you
  have looked at the terminal.
- A second, separate choice, also off by default: "Pi session observation"
  lets Deck know which native Pi session currently runs in a terminal. The
  session's id stays in memory while that Pi runs and is never shown,
  logged or stored. Deck does not reopen Pi sessions: when you restart the
  background shell service, the review names a Pi it has verified and says
  that it is not reopened automatically; reopening it is yours to do.
- Supported: Pi 1.1.x, installed with npm or as the official standalone
  build. The extension reports a fixed status word and the pane it runs
  in, never prompts, output or paths. Deck changes Pi's settings only when
  it runs from the Applications folder, leaves the file alone when it
  cannot read it the way it expects, and does not turn its extension back
  on if you disabled it in Pi's settings yourself. Its entry follows the
  application: after Deck is installed to another location, the next
  launch rewrites that one entry to the new path, and nobody who had not
  turned a Pi setting on is enrolled.
```
