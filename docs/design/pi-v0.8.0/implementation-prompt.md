# 交给独立执行 Agent 的完整 Implementation Prompt

你是 Deck v0.8.0 Pi 第一方集成的**独立实现与验证 Agent**。设计/验收负责人已完成本目录设计；你负责在范围内实施、自动测试、修复、收集证据和本地提交，不能自称最终验收人。设计负责人会独立审查最终diff及证据，并给Follow-up。

## 背景、任务和明确授权

Deck owns attention around work. The Agent owns the work.

请实现 **P1a：Pi工作开始/交互结束Signal与诚实覆盖说明**，**P2：原生Session生命周期观察**，以及**P3当前不可用的准确展示/拒绝**。继续完成所有可实施范围，不因P3阻塞停止。

原始P1要求可靠working/needs-input/turn-done；当前Pi1.1.0的ui_prompt事件已实测乱序，**完整P1 BLOCKED**。禁止生产使用其生成needs-input/clear。P3代码身份也已阻塞；**PI.reentry=None，不实现Pi自动恢复启动**。这两个阻塞不能在报告里改成PASS。

在本设计范围内，你可以自行调查实现细节、修改代码、构建隔离环境、修复测试设施和失败、运行static/unit/integration/native/WK E2E、安全回归、保留证据、清理资源及进行批准范围内的本地提交，不必逐步骤询问用户。用户不承担常规点击、命令、配置和测试。不能因一次失败停止；两次实质修复无效时重新定位根因，以新证据改变方案，不盲重试。

不得修改产品宪法、扩大功能范围、降低认证、绕过门禁、弱化断言、接触真实Pi/Deck配置/会话、操作无关项目、读取私人仓库、安装全局工具、改系统权限、launchd/TCC、安全设置、push/merge/tag/release/nightly/stable/部署或发布上游issue。需要这些不等于普通工作获授权；报告具体BLOCKED及已尝试的安全替代方案。

## 必须先读

1. 根目录 `CLAUDE.md`、`AGENTS.md`，及本实现涉及目录的任何额外AGENTS；以最新checkout为准，先`git status`保护已有编辑。
2. `docs/design/pi-v0.8.0/README.md`（Detailed Design，所有表和协议约束）、`decisions.md`、`acceptance.md`。
3. `docs/evidence/pi-investigation-20261012/README.md`、identity.json、events.json、peer-summary.json、原型TS。真实基线Pi1.1.0 commit `abe508e1b89912adde45528136c3221eb69acdd7`，不能用main替代。运行前复核官方release；若新版本改变关键契约，先保留此基线实现和认证，不自行放宽范围。
4. `docs/first-party-extensions.md`、`docs/codex-signal-certification-20261002.md`、`docs/codex-signal-certification-20261008.md`、`docs/scheduler-context-safety.md`、`docs/auto-respond.md`、`docs/evidence/closeout-20261011/reentry-acceptance.md`、`app/SMOKE.md`、`.github/workflows/gate.yml`。
5. 每个准备编辑的模块头部：`extensions.rs`、`agent_status.rs`、`agent_origin.rs`、`agent_reentry/{mod,claude,lifecycle,launch}.rs`、`procinfo.rs`、`procinfo/code_identity.rs`、`scheduler/{select,delivery,first_send}.rs`、`admission.rs`、`commands.rs`、`ui/js/{settings,reentry-model,restart-reentry,attention-model,pure}.js`；`tests/{edr_quiet,signal_census,log_privacy,external_admission}.rs`和`build/shared_output.rs`。

读前先定位；不要将全部源码反复打印。长日志写隔离报告目录，记录真exit code。设计调查基线Deck是`1bdcf57...`；若最新代码变动，列明会影响本设计的变化，不能覆盖别人的修改。你不是代码库唯一工作者；保留已有变更，写所有权限于本任务。

## 实现范围和文件归属

- 新Pi实现：`app/src-tauri/src/agent_pi.rs`，必要时其config/transport子模块；`resources/pi/`里的共用observer和三个薄entry variants。
- 静态接入：`extensions.rs`和AgentAdapter安装闭合enum；不要dynamic registry或泛化插件trait。
- shared Signal：最小扩展typed interaction key、Pi ingress/coverage投影；native-v1/v2和旧来源逻辑不重写。
- Lifecycle：仍由`agent_reentry/lifecycle`拥有观察/回执的严格分离；Claude-native decoder/reducers回归claude.rs；core的身份和deadline不动；Pi只有volatile观察。
- Scheduler：独立Pi input-unverified hold、fresh-start只启动不输入、pre-fire再次检查；不加remote admission、不加authority类型。
- UI：Settings、卡片有限覆盖、Restart混合不可选择原因、closed IPC、双语和现有测试。
- Packaging/test：bundle资源、现有build流程和EDR/privacy扫描扩展到TS；增加真实Pi测试及smoke mode/verdict，更新所属模块头部及相关文档。

不要编辑procinfo身份机制、agent_origin算法、tmux restart边界、Board自动动作、CLI产品版本、release配置，除非本设计要求的窄调用接线；发现需要改变时先记录具体新证据和设计差异，由负责人审查。禁止修改CLAUDE.md/AGENTS.md。

## 核心契约（不可省略）

1. 静态source `pi`；reentry=None；普通status v1/v2拒收Pi；不用status-helper传合成UUID。
2. 只从bundle加载Extension；用户settings.extensions的一个自有路径是两个capability开关的唯一事实源。signal/lifecycle/both三个常量entry共用一个实现；off/off无引用。保留用户字节、symlink、mode、负向禁用；malformed/重复key/不兼容结构不修改；boot只迁移已启用自有项。
3. 只使用Pi公共Extension API，mode=tui；不访问prompt、messages、title、tool/result、transcript；不调用sendUserMessage/sendMessage/newSession/switchSession/fork/executeTool/exec等执行API。测试fixture可为驱动真实Pi使用这些API，须明确隔离，不能打包。
4. 现有status.sock上的`pi-observer` family，严格bounded hello/seq/run/frame；直接kernel peer必须等于当前FG leader且有正确birth/ancestry/tty/pane/server。环境、版本、ID都是声明不是证明。
5. backend stream lease、观察者run counter与nativeSessionId是三个不同东西。一个generation同时只一个observer；旧lease数据不能提交。每帧保持当前generation再检查；重连不回放旧turn-done。
6. agent_start开新run或延续未settled的run；agent_end不用；agent_settled结束当前run；没有start不合成完成。不订阅生产ui_prompt；不模拟needs-input。
7. 配置/连接/兼容/覆盖分离；失联/禁用/溢出撤销对应观察并显示unavailable，不能静默成为safe。heartbeats只能证明transport，不创建Agent state/readiness。
8. 任何自动Pi输入恒HOLD，包括readiness override、先前interaction、clock/head/followup和pre-fire竞态。manual send保留正常guard。不会给Pi增加Slack/Phone/channel命令准入。
9. P2 ID按Pi字符集、≤256，不强制UUID；不传path/nativeID到日志或UI；session_start与shutdown具体reason保留，不凭quit断言exit。观察不能进入armed receipt mailbox。
10. Claude所有动态签名/文件generation、lease、serial、deadline、before/after re-read、queue、Board fence和once-only规则保持；未知Pi不能靠ReceiptContract表项变候选。
11. 信号只影响共享store/episode/attention和HOLD，不移卡、关闭、建立/恢复authorization、自动retry输入、判断任务成功。
12. 资源有界：32streams/128总待处理/每client32帧，单admission worker；line1024、2s ACK、5s ping、15s stale；无每事件fork/thread，无新listener；不能让Pi阻塞旧来源。

具体字段、重连、reset/EOF和生命周期snapshot规则以Detailed Design为准。若实现需要调整资源数值，在报告附负载证据和理由；不得把有界变无界。

## 阶段执行顺序

### E0：基线与隔离

记录实际HEAD、worktree、toolchain、OS、Pi artifact版本/source/hash。先跑相关现有窄tests发现baseline failure，再开始改动。除本任务docs外的已有编辑不得提交。不要先把全部代码大改后再验签/测试。

所有native测试用新临时root、HOME、PI_CODING_AGENT_DIR、workdir、session-dir、TMUX_TMPDIR/full socket、Deck data；不继承auth/env凭据。只用bundled tmux（可复制到临时root以符合registry规则）。长期server/process启动时登记，停止只用`edr_runtime.py experiment`；不能`tmux -L deck kill-server`、pkill或listing PID kill。registry拒绝时诊断，不绕过。

Mac mini非必需；如使用先实时inventory，再按AGENTS清理/恢复。开发机有可用native设施优先本机隔离。Pi npm runtime只读复用可用安装，官方standalone只下载到临时root；不全局升级。

### E1：P1a最小实现与安全闭环

实现注册、bundle资源、lossless installer、direct stream、归属/ordered ingress、共享Signal投影、Pi automatic hold和UI有限覆盖。先用unit/integration完成wrong-pane/old-generation/reload/overflow/disable/no-replay负例，再真实Pi TUI+两个pane验证。不得在此阶段顺手加入needs-input或lifecycle原生ID。

完成真实Settings enable→临时config→Pi加载→production socket→Board/Attention→disable整个闭环。开发build一般不准写全局配置；复用隔离debug/relocated-home机制，不能新增生产绕过参数。未通过E1不宣称P1a交付。

### E2：P2与Claude receipt解耦

添加独立lifecycle开关/entry组合、native current observation和source-specific ID。按设计提取Claude decoder/reducers，保留每个receipt fence；P2 frame不走代码身份worker、不喂reentry mailbox。执行全部Claude regression及混合来源测试。

真实new/resume/fork/reload/quit与异常退出/失联矩阵、signal-only/lifecycle-only/both/off配置组合、同session两个client、不重放旧session关联都要有证据。nativeID只在临时测试日志中比较，提交证据匿名化。

### E3：P3拒绝、UX与完整gate

Pi卡片显示确切不可用理由；不可选择、伪造request也不能启动，不影响Claude选择/Policy B/队列暂停。用真实官方ad-hoc Pi和Node安装作为negative identity证据，不创建Pi digest表。不要写Pi launch占位代码。

补完整WK、packaging资源校验、EDR/privacy资产扫描、full gates，清理资源，再review diff/local commits。P1完整与P3保留BLOCKED，E3negative checks通过不等于P3成功。

## 必跑验证与自动验收

完整矩阵在acceptance.md，至少：

- Rust：`cargo fmt --all --check --manifest-path app/src-tauri/Cargo.toml`；`cargo clippy --workspace --all-targets --all-features --manifest-path app/src-tauri/Cargo.toml -- -D warnings`；`cargo test --workspace --manifest-path app/src-tauri/Cargo.toml`。生产build/source变动按gate.yml运行对应release/provenance/edr检查；不要因名称不同漏workspace helper。
- Frontend：所有修改JS/MJS语法检查；`scripts/ui-tests`；从适当目录执行`node app/ui/js/check.mjs`（若脚本要求cwd则遵循其现行入口）。TS资源不属于既有JS扫描覆盖，补等价语法/打包/禁止API测试，不能以未被扫描当通过。
- Safety：signal_census、edr_quiet、log_privacy、external_admission、procinfo、reentry现有tests以及registry regression；新增Pi fixtures必须被扫描器分类，不能放宽扫描规则掩盖新spawn。
- Native：真实interactive Pi npm和Bun1.1.0，deterministic provider只替模型响应；不能用fake_reentry_agent/直接调用handlers冒充真实Pi。必须让Pi自己触发retry/follow-up/settled，保留UI乱序负例作为BLOCKED证据。
- WK：通过`app/run.sh`/LaunchServices，绝不后台裸跑deck-app；fresh smoke dir/socket；新增`pi-live`类场景调用真实backend及Pi，使用smoke-verdict。Settings和Restart layout mock另记，不能代替native E2E。
- UI操作用现有WK smoke/native设施，或已可用UI自动化；不要求用户常规点击、不新增release测试后门、不reset TCC。

为每条AC保留可追溯结果。没有凭据的Claude额外provider live认证可单独BLOCKED，但既有unit/controller/WK fixture回归必须自行执行；不能说“无模型账号”就跳过真实Pi（本地fixture不需账号）。不要求生产发布、notarization、installed path覆盖才能做隔离验收；但不要把debug结果称release结果。

## 成功标准与BLOCKED规则

成功实现范围：P1a/P2所有AC通过、Pi始终无自动恢复入口、完整旧gate通过、实际Pi native+WK链路有证据、环境清理完成。整个P1/P3仍依原设计BLOCKED，报告不可合并为“Pi全部PASS”。

新增BLOCKED只用于有外部前提且证据充分：上游缺原语、系统版本/权限能力确实缺失、不可取得合法签名分发。常规编译错误、一次test失败、不知道如何驱动UI、超时或fixture bug都不是立刻BLOCKED理由。先定位、最小修复、重新验证；不可修复时说明被阻AC、失败命令/日志、尝试过的安全替代和不受影响范围。

如果发现设计中不可安全实现的具体矛盾，停止该依赖分支，不自行降级；继续独立部分，并向设计负责人提交问题编号、根因证据、建议最小设计变更。不要把决定转给用户做工程选择。

## 提交和返回报告

可做本地原子提交，建议分别P1a、P2、验收/文档；不push，不改版本号。每提交行为与owning header同步；tests不省略，不能提交敏感runtime日志/凭据/Session正文/ID路径。测试fixture只能包含合成内容。

返回一份可独立审查的报告，含：

1. 最终commit(s)、实际base、worktree状态、全部修改模块及理由；设计偏差逐项说明。
2. P1a、完整P1、P2、P3分别PASS/FAIL/BLOCKED；Security、Regression、Native E2E、Product Constitution同样分别给判定，UNRUN必须显式说明且不能计PASS。
3. 每AC的证据路径、命令、exit code、实测断言；区分unit/integration/real Pi/WK mock/WK production/signed release。
4. Pi版本/source/artifact hash、两种进程形态、local-provider界限；旧Claude/Codex regression范围。
5. 关键攻击负例：跨pane、继承环境、旧generation/lease/run、重复序号、untrusted/replaced executable、UI乱序、自动发送override仍HOLD。
6. 原始失败与修复后的结果，不能删除失败记录；测试夹具修复单列，避免包装为产品故障或抹掉产品失败。
7. 隔离root/registry清理exit code、最终no-owned-process/socket审计、真实配置未改证据；长日志private，提交证据sanitize。
8. 未解风险/BLOCKED、后续解除所需证据、最终diff自审结果。

不要仅写“所有测试通过”。最终验收属于设计负责人；收到Follow-up时，按问题编号完成最小修复、必要完整回归和证据更新，再返回报告。
