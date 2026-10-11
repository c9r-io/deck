# Pi 第一方集成调查证据（设计阶段）

调查时间2026-10-12（Asia/Tokyo）；Deck与Pi精确对象、官方artifact摘要、源文件摘要见[identity.json](identity.json)。设计与实施边界见[详细设计](../../design/pi-v0.8.0/README.md)。**没有修改Deck生产代码、用户配置、真实会话；没有执行完整功能实现、提交、push或发布。**

## 调查方法与证据等级

- Deck：读取当前checkout宪法、模块契约、生产调用链、现有安全测试及v0.8验收文档。已有Claude/Codex证据作为约束，不当成本轮新测试。
- Pi：从[官方站点](https://pi.dev/)确认仓库，查[官方v1.1.0 release](https://github.com/earendil-works/pi/releases/tag/v1.1.0)，checkout release commit而非浮动main；读取原生事件分发、settlement、session resolver、loader、构建脚本。
- Runtime：真实Pi1.1.0 interactive TUI；一项npm/Node、另一项官方Bun独立binary，在两个隔离tmux pane的同一临时workdir中并发。模型通过测试Extension的本地deterministic provider返回合成响应，未调用远程模型、未使用/复制用户credentials。
- 传输：测试Extension通过node:net直接发one-shot Unix stream；collector用macOS LOCAL_PEERPID读取peer，再采样其parent/PGID/foreground/tty与实际pane。**这不是Deck生产ingest，也不是设计中的persistent stream。**
- 源语义复核：独立架构审查Agent只读检查UI prompt乱序/settlement问题；本轮设计者随后以真实TUI复现。没有委派实现。

## 保留的证据

| 文件 | 内容与隐私边界 |
| --- | --- |
| [identity.json](identity.json) | Deck/Pi commit、OS、runtime版本、artifact/source SHA256、codesign分类；无私人安装路径 |
| [events.json](events.json) | 实际event trace；pid/instance/nativeID/path分别一致匿名化，保留相等/变化关系；无prompt/title/output/tool内容 |
| [peer-summary.json](peer-summary.json) | 每次peer采样的逐项布尔核验；无真实PID/tty路径；不是伪造的production trust判定 |
| [probe.ts](probe.ts) | 调查fixture；本地provider、驱动命令、内容外metadata recorder；不是产品代码，不能打包 |
| [delayed-ui.ts](delayed-ui.ts) | 较早Extension延迟start、自动关闭无害confirm的确定性反例 |

原始shell日志/事件/collector/实验registry与上游checkout位于本机临时实验根`/tmp/deck-pi-iJiyNU`；这是临时可核查材料，不是checkout可依赖的长期路径。可移植结论由上表的匿名证据、固定source和fixture支撑。截图/终端capture可能含测试机hostname，未纳入仓库。

## 实际结果

| 测试 | 结果 | 能证明 / 不能证明 |
| --- | --- | --- |
| 官方稳定release、checkout、archive SHA256 | PASS | release1.1.0、commit abe508e…，archive与官方SHA256SUMS一致；不证明可复现构建/运行代码供应方身份 |
| 官方arm64 Pi code signing | BLOCKED for P3 | Identifier=a.out、Signature=adhoc、TeamIdentifier未设置、无sealed resources；无法满足现有供应方要求 |
| npm运行图像 | BLOCKED for P3 | runtime为Node24.19.0，Node.js Foundation签名不覆盖Pi JS；process.title可以变成pi而tmux仍显示node |
| 两真实TUI并发 | PASS（原型） | 两个native进程、不同pane同cwd；Node和Bun均直连socket，独立start/settled |
| 49个peer样本 | PASS（拓扑） | 每个kernel peer=对应日志client；parent=声明pane shell、peer=PGID=tty foreground、tty一致；非Deckingest/非PIDbirth完整认证 |
| 正常run | PASS | start→end→settled；不含task success判断 |
| retry | PASS | start/end重复，只有最终一个settled |
| agent_end排follow-up | PASS | 两段低层run、一个settled |
| settled handler启动新run | PASS | 两个settled之间有新start；不是提前结束旧自动重试 |
| confirm/input/select | PASS（事件存在） | ctx.ui wrappers发start/end；不说明完整UI coverage |
| 上游乱序反例 | FAIL（可靠等待契约），完整P1 BLOCKED | 较早handler delay1500ms、confirm timeout100ms，观察者end→start |
| 内置/model UI | PASS（缺口验证） | 原生selector打开关闭没有ui_prompt事件；不能宣称全部UI可观测 |
| new/reload/fork | PASS | nativeID变化/保持、不同factoryinstance、shutdown reason准确 |
| CLI现存path/完整ID恢复 | PASS（Pi行为） | nativeID相同，start reason均startup；不是DeckReentry确认 |
| runtime switchSession | PASS | shutdown(resume)后start(resume)，目标ID与原session一致 |
| --session missing path | PASS（危险行为验证） | 创建新ID，file可尚未落盘；因此不是fail-if-missing恢复 |
| --session-id自定义ID | PASS | `probe.custom-ID_123`原样接受；不能强制UUID |
| 正常quit | PASS | 两种runtime有shutdown(quit)，进程随后消失 |
| fixture错误引起异常退出 | OBSERVED | 未给switchSession传目标导致Pi异常退出，无shutdown事件；只证明不能假定总有shutdown，不是计划内crash矩阵认证 |
| persistent observer / install / production Deck Signal / WK E2E | UNRUN | 本阶段不实现产品；交给独立执行者按清单验收 |
| 完整editor/custom/嵌套UI、compaction、所有builtin/authUI、code-replacement runtime | UNRUN | 源码约束已读，运行矩阵仍在implementation acceptance |

原始P1 BLOCKED与P3 BLOCKED互不依赖。以上PASS都注明对象；不能提升成“Pi完整支持PASS”。

最终base campaign记录51条source事件、collector取得49条peer样本；两个最终quit的异步one-shot发送未被collector采到。shutdown的原生事件存在性由source日志证明，不能声称其socket回执通过。全部49条实际peer样本六项断言通过；这也是设计要求正式transport提供bounded shutdown drain并对缺回执保持unknown的原因。完整四组最终campaign合计75条匿名source/fixture事件。

### 关键匿名事件轨迹

正常retry与follow-up各观察到：

```
agent_start → agent_end → agent_start → agent_end → agent_settled
```

settled handler显式开始后续工作：

```
agent_start → agent_end → agent_settled
agent_start → agent_end → agent_settled
```

真实乱序反例（含fixture控制标记）：

```
fixture_start_delayed
fixture_end_received
ui_prompt_end
fixture_ui_resolved
ui_prompt_start
```

最后start指的是已经关闭的同一个弹窗。生产observer没有能识别它属于旧请求的source ID/revision。JSON里的observer seq只是收到后的序号，不是上游产生次序。

## 复现说明（供执行Agent，不要求用户操作）

1. 新建短临时root，`edr_runtime.py experiment init`；HOME、PI_CODING_AGENT_DIR、workdir、sessions、TMUX_TMPDIR都在其中。只用现有bundled tmux副本，完整`-S`socket，启动后立即add-server；长寿命Pi add-process，停止只走registry。不要复用默认tmux socket。
2. 真实Pi1.1.0，`--no-skills --no-prompt-templates --no-themes --no-tools -e <root>/probe.ts --provider deck-probe --model fixture --session-dir <root>/sessions`。临时settings禁用builtin:mcp/llama.cpp/codemode/tool-search，defaultProjectTrust=never，不复制用户config。使用空白测试cwd。
3. 设置PROBE_LOG到private JSONL、PROBE_SOCKET到private collector。collector LOCAL_PEERPID读peer，采样ps的metadata列（只测试，不进产品），核对指定pane的pane_pid/tty。record包含临时nativeID/path，保留前必须匿名化。
4. 通过测试tmux literal输入普通fixture prompt、/probe-retry、/probe-follow、/probe-settled、/probe-ui、/probe-input、/probe-select、/probe-new、/reload、/probe-fork、/quit。UI等待用Escape取消；发送新slash命令先清除fixture已有composer文本，fork会填入旧prompt，不能把/quit拼在后面。
5. 对source乱序，将delayed-ui.ts以排序靠前的名字`00-delayed-ui.ts`加载在observer之前。执行/probe-auto-ui，100ms timeout后等1500ms delayed handler释放；检查上述end→start，而不是只看有两个event。前一个Extension只是普通异步handler，没有修改Pi源码。
6. 现存session恢复分别用--session path及完整ID；记录startup。运行中resume测试需在**新Pi的launch env**传PROBE_RESUME为测试session的path，不能仅修改已经运行shell之外的tmux全局env。自定义ID与missing-path分别启动独立Pi，不调用最近session。
7. 正常退出，cleanup registry，复查root没有ownedprocess/socket。保留精确exit code、原始失败和最终重跑结果；不要把collector当productioningest。

## 测试设施问题与修正记录

透明保留，均没有修改生产代码：

- 第一次尝试把同步测试controller的系统Python登记，registry正确拒绝其`/Library`路径，exit3；尚未启动tmux/Pi。没有绕过registry停止该Python。后续controller为有限同步命令；长寿命资源使用临时root的bundled tmux/Node副本或官方Pi并登记。
- 初轮fork后composer已有文本，/quit拼入prompt，quit等待timeout（exit1）；以测试输入清空composer后正常退出。fixture修正send前清空，最终base campaign exit0，两client正常quit。
- 初次slow.ts排序在observer后，未能复现乱序；查loader后命名00-slow.ts，真实得到end→start。无须改Pi源码。
- 初次runtime-resume fixture未把env传入已存在shell，传了undefined路径，Pi异常退出；修正新client的launch env，重新完整builtin/resume案例exit0。
- 独立复核发现初轮collector frame里的seq在connect回调读取了可变闭包值，20个样本不能按event/seq一一匹配（peer/parent/tty证明仍在）。改为捕获`data.seq`，完整重跑base campaign exit0；保留的最终peer证据49/49全部逐事件匹配。没有把初轮序号错误归咎于Pi。

## 隔离和清理

实验只在临时root运行；没有Deck app启动，没有user HOME/config写入，没有模型auth、系统权限或Mac mini改动。只读复用本机npm已安装的Pi1.1.0代码，运行的Node复制到临时root；官方Bun artifact下载到临时root；tmux为checkout中已有bundled二进制副本。

完成一轮后通过experiment registry cleanup，再复查注册PID均gone，最后socket不存在；最终清理结果见[closeout.json](closeout.json)。临时无凭据的源码/fixture/合成session和原始日志可保留核查，不作为产品安装。没有通过-L socket名、pkill、任意listing PID清理资源。

本轮repo改变仅本设计目录和本证据目录。Rust/frontend/product release gates对纯调查文档变更不适用；它们明确列为执行阶段必跑，未声称本轮通过。
