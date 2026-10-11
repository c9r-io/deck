# Pi v0.8.0 独立验收清单

本清单由设计/最终验收负责人使用，不能仅凭执行者报告的PASS勾选。对每项填写**最终commit、证据路径、实际命令/exit code、期望与实测、独立复核方式、PASS/FAIL/BLOCKED**。未运行写UNRUN，视为未验收；不是PASS。完整P1和P3的阻塞不得由下列拒绝测试的PASS替代。

## 验收入口

先核对`git status`、base→final完整diff（包含TS resources、test harness、packaging），不是只读摘要。重新读涉及的模块头部与CLAUDE.md。独立验证报告artifact真实存在并与最终源码一致；截图只能证明可见内容，不能单独证明native attribution/zero-input；模拟后端截图不能证明真实IPC。

证据种类：U=Unit；I=Integration（真实socket/process/tmux，允许fixture）；N=Native Runtime（真实Pi TUI，模型可为隔离deterministic provider）；W=真实WKWebView production IPC链；L=WK mock/layout；S=Safety/Regression。必须分列，不能互相代替。

## P1a：注册、配置和有限Signal

| AC | 种类 | 场景 / 必须结果 | 最小证据 |
| --- | --- | --- | --- |
| A01 | U/S | 静态registry恰好新增pi；重复source无赢家；registration没有权限副作用 | registry测试和descriptor diff，PI.reentry=None |
| A02 | U/I | source=pi不能走普通v1/v2；nativeUUID原规则不变 | parser拒绝及Claude/Codex已知向量 |
| A03 | U/I | hello/frame未知key、重复key、超长、超界整数、错误version/mode拒绝 | adversarial wire fixtures；实际listener不挂起 |
| A04 | U/I | signal/lifecycle/both/off配置切换；不写home可执行文件；stable位置guard | exact前后文件bytes/mode/symlink、bundle resource清单 |
| A05 | U/I | 用户extensions/packages及未知settings原字节保留；malformed/重复key/非数组不写 | byte equality；真实临时文件测试 |
| A06 | U/I | 移动bundle迁移只改Deck项；用户-negative保持禁用；无旧项不enroll | migration矩阵和config source-of-truth读取 |
| A07 | U/I | 并发配置变化不覆盖已检测冲突；双开关不留下两个observerentry | race fixture；明确最后rename残余 |
| A08 | N/W | Settings实际开启→Pi从该config加载→真实production hello；已配置≠已连接 | WK checkpoint、config hash、生产closed诊断，非mock |
| A09 | N/I | npm/Node和官方Bun两种TUI，peer就是FG leader、parent是本pane shell、tty连续 | runtime版本/hash、kernel sampling、实际ingest成功 |
| A10 | I/N | 两个Pi、两个pane、同cwd；A/B并发互不串状态；非active pane不污染card | independently captured pane→lease→run匿名trace |
| A11 | I/S | 伪造pane/server/socket、自报PID、继承TMUX_PANE的后台child、非TUI均不能命名别的卡 | negative admission真实kernel peer；被冒名pane不受影响 |
| A12 | I/S | 旧FG、PID复用fixture、exec/replaced peer、slow validation后generation变动 | delayed commit fence tests；旧state不覆盖新 |
| A13 | U/I | duplicate seq、gap、run回退、late done(A)对B、old lease callback、同FG双observer | state/episode精确断言，无last-wins |
| A14 | N | 正常run start→settled；仅working→turn-done，不把loaded/idle当done | 真实Pi事件与生产Signal trace对齐 |
| A15 | N | retry / queued follow-up / before-settle continue，中间agent_end不产生done | 每种真实runtime至少一次，model fixture控制响应；episode计数 |
| A16 | N | settled handler发起新run，是done(A)→working(B)→done(B) | 真实顺序、不同run key，无time debounce伪合并 |
| A17 | N/I | abort、provider error结束仍只表示interaction边界，不标task success | closed状态和UI文案；无成功判断 |
| A18 | N/S | UI confirm/input/select/editor/custom、嵌套/并发prompt；生产不伪造needs-input | fixture真实产生UI，production无此状态；coverage提示存在 |
| A19 | N/S | earlier-extension delayed start/end、跨A/B、跨settled；复现BLOCKED反例 | 完整源序/观察序匿名证据；没有“本地UUID修复” |
| A20 | N | 内置/model、至少一个启动/内置非Extension UI场景 | 无event也诚实提示；未测的其他UI明确UNRUN |
| A21 | I/N | missing/--no-extensions/negative resource/加载抛错，没有hello | status unavailable，不能诊断为已加载、不能合成done |
| A22 | I/N | handler内部错误、EOF、socket断开、过载、15s失联 | 撤销own episode、低干扰coverage、无泄漏/重试历史done |
| A23 | I/N | Deck重启、Pi仍运行；重新hello；idle不重放旧done，active可重新working | 实际observer重连；旧authority/evidence不继承 |
| A24 | I/N | 32连接上限、128队列、每client32、slow reader、partial line、flood | 资源计数/响应有界；Claude/Codex同时信号不饥饿 |
| A25 | W/S | 关signal立即撤销后端观察；旧Pi未reload不得继续写Signal | 真Settings/IPC、旧client继续活动负例 |
| A26 | W/L | 卡片、Needs attention、away/Dock、viewed episode规则复用；coverage本身不变urgent | shared trace+WK；未获通知权限项目单列BLOCKED |

P1a PASS需要A01–A26按适用项均关闭；替代测试须说明能证明同一断言。**完整P1**还需要可靠needs-input/clear的上游新原语和新设计复核，现行1.1.0仍BLOCKED，不能因A18/A19负例通过给完整P1 PASS。

## 自动化安全（P1a必需，不是额外自动化功能）

| AC | 种类 | 必须结果 | 证据 |
| --- | --- | --- | --- |
| Q01 | U/I | 目标hint=pi / foreground=pi / 当前generation有Pi观察，各路径自动行HOLD | select、delivery、tick、pre-fire、atomic delivery调用计数 |
| Q02 | U/I | working/done/hello/ping/session snapshot均不解除hold、authority不变 | queue attempts=0、ledger无firing、实际pty输入=0 |
| Q03 | U/I/N | clock、approved followup、现有first-send readiness override、channel head不能给Pi豁免 | saved授权fixture +最终guard；不是仅UI禁用 |
| Q04 | I/W | fresh Pi可启动shell/client但同次/后续pass没有prompt字节/Enter | actual tmux输入审计和pending row |
| Q05 | U/S | Pi不加入remote allowlist；Node进程名不当Pi identity/target替代 | external_admission tests；无任意node放宽 |
| Q06 | I/W | manual Send now原guard保持；错误target/foreground/paste-edge拒绝，部分paste仍ambiguous不retry | actual delivery返回和ledger |
| Q07 | S | Signal不移卡、不close、不改变content authority/trigger、P2不影响readiness | signal_census全部消费者与diff独立审查 |

## P2：原生Session观察

| AC | 种类 | 必须结果 | 证据 |
| --- | --- | --- | --- |
| B01 | U/I/W | signal-only不读取/发送native ID；lifecycle-only不产生Signal；both一个stream | payload schema/fixtures和真实config加载 |
| B02 | U/I | Pi合法非UUID ID接受；Claude仍严格UUID；source+ID不混淆；超长明确unsupported | parser vectors和memory型别 |
| B03 | N | startup创建、CLI --session现存path/完整ID恢复、运行中resume、new、fork | 匿名ID一致/变化关系；reason精确，不猜latest |
| B04 | N | CLI resume的startup不能伪作resume回执；不存在path创建、不存在ID失败、session-id创建 | actual exit code和文件header关系；合成测试数据 |
| B05 | N | reload同ID但不同instance/lease；old callback无效；new/fork/resume旧runtime teardown | before/after标记、旧连接拒绝 |
| B06 | I/N | 两pane同session ID仍为两个process观察，不合并；same cwd无关联权 | stream/pane并发trace |
| B07 | N/I | quit只是runtime teardown；正常OS退出与crash/无shutdown分开 | kernel process exit+receipt存在/缺失；无虚构quit |
| B08 | I/N | off、disconnect、foreground变更、Deck重启清理memory；不读磁盘恢复观察 | stores/poll exact断言；disk/log无nativeID |
| B09 | U/I/S | Pi observations永不进Claude receipt mailbox；lease未armed也只存允许观察 | protocol isolation和type boundary源码检查 |
| B10 | U/I/S | receipt contract解耦保留Claude decoder/final/entered semantics；所有旧deadline/fence/overflow测试过 | 现有tests真实重跑及最小diff |
| B11 | I/S | 慢/挂起Claude Security调用不阻塞Pi和其他Signal；Pi洪泛也不阻塞旧receipt worker | timing上界、有界worker/queue计数 |
| B12 | W | lifecycle toggle +当前观察可用性/Restart reason真实投影；ID/path不出UI | production IPC response、WK截图/trace |

P2 PASS表示“当前会话观察”，绝不表示old/new client产品身份通过。B03的精确原生恢复只验证Pi CLI行为，不认证Deck自动Re-entry。

## P3：当前拒绝与未来解除门槛

| AC | 种类 | 当前必须结果 | 证据 |
| --- | --- | --- | --- |
| C01 | N/S | npm Node签名和JS身份分离；官方Bun ad-hoc不能满足Pi产品认证 | artifact checksum+codesign原始输出私存、sanitized identity；无digest allowlist |
| C02 | U/I/W | Pi.reentry=None；fake approval/token/混合review均不产生Pi exit/launch | backend调用计数=0、实际PTY零启动 |
| C03 | W/L | Pi不可勾选且原因准确；Claude照常候选；Policy B不传播到Pi；无需读session ID | 中英文真实review截图+request检查 |
| C04 | S | 原Claudeonce-only/freeze/Boardfence/动态image identity/late receipt/code replacement都不变 | 全部现有reentry/procinfo测试与diff独立核对 |

未来P3 PASS还必须具备供应方运行代码证明、可冻结精确持久化目标、最终退出回执+OS退出、支持的安全launch grammar、只一次实际请求、新running code/argv/环境/目录以及原生精确目标确认、timeout/ambiguous零重试。缺任一项BLOCKED；不得执行本轮范围外的Pi自动launch实验后直接开放能力。

## 全局gates、真实应用与隔离

| AC | 种类 | 必须结果 | 证据 |
| --- | --- | --- | --- |
| G01 | S | fmt/clippy全workspace all-targets all-features无warning；workspace tests过 | finalcommit commands+exitcode+完整私有log |
| G02 | S | frontend syntax/ui-tests/check全过；新TS资源实际在扫描范围 | gate logs、扫描器正负控制 |
| G03 | S | edr_quiet/log_privacy/signal_census/external_admission/provenance全部过 | 不删断言、不略过新资产的diff审查 |
| G04 | S/I | Claude Signal/Reentry、Codex legacy/shared unavailable/embedded归属不回归 | 原测试重跑；缺模型账号的live项单列 |
| G05 | W | 真WK+productionbackend+真实Pi；Settings安装、状态/Attention、disable、混合Restart | app/run.sh隔离参数、smoke-verdict exit0、checkpoints |
| G06 | L/W | ≥1280×800、中英、fontscale、长标题、mixed/noCandidate/failure状态布局 | 实际截图，mock与native分别标识 |
| G07 | S | bundledresource版本/路径/签名归属、打包不包含probe/provider fixture | release/debug manifest、sidecar/resource provenance检查 |
| G08 | S | 临时HOME/agentdir/session/workdir/Deckdir/full tmux socket；无真实data变化 | 前后hash/inventory（不输出凭据），registry完整 |
| G09 | S | 停止只从registry；cleanup后无ownedprocess/socket；拒绝不绕过 | cleanup exit0 + final audit，失败轨迹保留 |
| G10 | S | 没改宪法、没发消息/push/tag/release、没新任务语义/工具授权 | gitdiff、commands audit、生产API扫描 |

发布包或其他OS无法自动测试时对应项BLOCKED并注明范围，不代表G05可以跳过。不要用本设计轮的collector PASS充当执行版G05。

## 最终独立判定模板

```
Design Verdict: PASS / FAIL / BLOCKED（按实际设计变化重新核对）
Implementation Verdict: ...（只判获准范围，写清P1a/P2）
P1a Run-boundary Signal Verdict: ...
P1 Full Signal Verdict: BLOCKED（除非新上游证据及设计批准）
P2 Lifecycle Observation Verdict: ...
P3 Re-entry Verdict: BLOCKED（现行身份与Session前提）
Security Verdict: ...
Regression Verdict: ...
Native E2E Verdict: ...（真实Pi/productionWK/其他矩阵分别列）
Product Constitution Verdict: ...
Remaining Risks: ...
Remaining Blockers: ...
Final Recommendation: ...（不得写成所有Pi能力完成）
```

## Follow-up治理

发现缺陷时生成可直接执行的Follow-up，沿用同一执行Agent、同一隔离和授权边界；负责人不直接接管实现。每问题写：`F-ID / severity / 具体表现 / 文件行或证据 / 根因判断（事实与推断分开）/ 违反AC及设计 / 最小预期修复 / 必跑回归 / 成功证据`。

明确要求“确认根因，在范围内修复并自动验证，提交完整证据”，不是只让对方调查。禁止无关重写、平行状态机、改已通过的安全契约。设施问题同样由执行者修复，不交用户点击。返回后负责人重验失败AC及其依赖回归，检查新的风险；仍失败继续收敛Follow-up。真正上游/环境BLOCKED保持明确，不把它伪装成工程任务或无限重试。
