# Board 与授权数据边界治理

2026-10-13；基线 `e1bd78eae7f6fdd1c63a276e7d00fd3857a15c79`，开始时工作树干净。
本轮是本地未提交改动；最终文件哈希与命令退出码见 `results.json`。
环境：macOS 27.0.1 (26A434) arm64，Rust 1.97.1，Node 24.19.0。

## 基线与范围

- 上一轮 `technical-debt-20261013` 的 hooks 拆分、fixture 同步、剪贴板 reader 修复已在
  `e1bd78e` 提交，本轮不重复处理。历史 tmux CI 超时仍待后续 CI 观测，未推送或触发 CI。
- Board 的混合职责与 channel head 重复解释 JSON 仍存在，本轮只收窄这两处边界。
- 已接受的设计约束：no-build ES modules；视图核心 Board/layout/terminal 的既有静态循环；
  shared build output 由 provenance 检查保护；授权未知时拒绝自动副作用。
- 上游限制：Pi needs-input/re-entry 与 Codex shared-daemon 归属缺口保持原状态；
  旧原生证据中的 BLOCKED、UNRUN 不由这轮测试关闭。

顺序：先确认授权测试缺口、补回归、引入 typed head 并通过相关测试，随后抽取一个卡片
展示职责，再运行完整适用门禁。没有并行重构 Board、scheduler 和 storage。
一个只读 agent 定位授权覆盖缺口；一个实施 agent 只负责 late-revocation 测试文件，
主 agent 完成产品实现、整体验证与验收。

## 授权边界

`documents::frozen_channel_head` 从当前 Board 中已经匹配唯一原生 origin 的 card 投影
`FrozenChannelHead`，复用 `BoardCard`、`ChannelRun`、target/claim/step 文档类型。
channel first-send 的实际 `frozen_matches` 消费该类型，不再逐字段解释 head JSON。
投影是只读证据，不是 grant、缓存、IPC 或持久化格式；无 Serialize 写回路径。
其余来源和 follow-up 的数据路径没有一起重写。

尤其保留 `initialQueued` **明确为 false** 的要求。旧文档可缺省该字段并通过解析，
但解析器的 false 默认值不能成为“从未入队”的证据。投影仅从原快照读取，不再次读 Board，
不改变 settings → Board → queue → save lock 顺序或 admission/pre-start/pre-fire 检查时点。
未知扩展字段仍由原字符串保存，恢复来源仍不可授权，永久 channel constraint 不可降级。

回归覆盖：

- 合法基线；请求身份/目录/时间/template；claim 四字段；Board origin、身份、来源、目标、
  冻结 claim、冻结 head 各自单独改变；缺省 initialQueued、重复 origin 均拒绝。
  扩展字段不影响合法准入，投影后原 committed 值不变。
- 既有 current/recovered、settings 不可读、撤销/重新发 grant、legacy/follow-up 与
  ambiguous 不自动重发测试继续使用，未替换为只测新结构的断言。
- tick 的 candidate 循环移至 `channel_first_send::revoke_stale`，继续复用 `Standing`。
  测试调用真实 `with_queue_opt`：未知/当前不写；撤销写失败不提交内存；成功只撤销一次；
  来源重新有效不能恢复已撤销 row，普通 owner row 不变。线程保留锁、读取、失败后不发送。
- 真实隔离 settings/Board/queue 文件、生产 `send_one_safe` 与 fake context probe：
  分别在 prepare 和 final_probe 中撤销。断言二者确实执行、fire 从未执行，撤销及 revision
  写入队列且 reload 可见；第二次自动调用不再 prepare/fire。原 pre-start 和 firing-intent
  先于副作用的检查保留。此测试不启动 tmux、Agent 或远程服务，不是实际输入认证。

## Board 的一个职责

`board-card.js` 拥有完整卡片 DOM、标签/chip、事件隔离与动作路由；`BoardCardActions`
及 card 参数有局部 JSDoc。`board.js::cardEl` 组合现有 action owners，维持所有调用者接口。
新模块不反向导入 Board/layout/terminal，也不拥有 provider、IPC writer、poll 或生命周期。

实施前后的唯一入口与顺序：

| 边界 | 保持的 owner / 顺序 |
| --- | --- |
| 持久化 | `persistence.js` 的唯一 serial transaction queue → `save_board` |
| 创建与关闭 | `provider.createStarted`：启动后保存，失败回收；`provider.close`：统一取消/关闭/写入及部分失败表达 |
| 退休 | 原 shell-exit tracker 与 automation finish tracker，仅通过 `provider.close`；Reminder、retained buffer、MCP、正在查看 pane 的保护不变 |
| poll | single-flight、一个事件 follow-up、epoch 失效；记录观测 → 更新 runtime/chrome → queue hints/plans → exit retirement → epoch 复核 → run retirement → tabs/attention/notify |
| 视图依赖 | Board/layout/terminal 原 SCC 不变；Board → board-card → 原 painter/model 依赖，不扩 allowlist |

原 `cardEl` 主体与新 renderer 做了逐字比较：除函数签名及 open/close/pin/context 四个
action 调用名称外相同。provider 区块及完整 polling 区块与基线逐字相同，CSS 未改。
新的 DOM 测试覆盖固定形状、textContent、独立 status/reminder/queue slot、子控件不打开
会话、拖拽仅传 identity、不提交 placement，以及 pin 并发点击/成功/失败时保留 focusable
与清除 busy。静态 tripwire 随真实 owner 移动，断言未删；coverage exclusion 未增加。

## 验证与失败记录

完整结果以 `results.json` 为准。本机原始日志在 `/tmp/deck-governance-*.log`，不作为
其它检出的运行依赖，也不将日志中的环境细节搬进仓库。

- 首次 targeted cargo 命令误用 `--lib`，exit 101（本包只有 bin target），改为
  `--bin deck-app` 后在原实现上通过新增准入回归。没有以重跑掩盖产品失败。
- late-revocation 测试首次编译 exit 101：缺少 `ProbeResult` 导入；补齐显式 import 后通过。
- 授权最终 targeted suite：81 passed，exit 0，含 tick 和 late-revocation 新场景。
- 前端：786 passed，0 failed；行 94.34%、分支 89.60%、函数 87.13%；新 renderer 三项 100%。
  156 个 JS/MJS 语法通过；62 模块静态检查通过，循环仍限于原视图核心。
- Rust fmt 和 workspace/all-targets/all-features clippy `-D warnings` 通过。
- Python release/registry/verification tooling：181 tests，exit 0。未修改的 tooling 测试
  输出了 12 条 unclosed-file ResourceWarning；保留原始日志，未抑制警告或扩展到无关 fixture 清理。
- 完整 Rust workspace/all-targets/all-features 串行运行：1,520 passed、0 failed、
  13 ignored，exit 0。ignored 均为原有专门环境/工具入口，本轮未增加或跳过用例。
  产品逻辑在启动完整门禁前已经固定；随后仅补 thread owner 注释、文档及 UI 静态边界断言，
  最终 UI gate 在该断言加入后完整重跑。最终构建检查另列于 `results.json`。
- release 构建、EDR 二进制词汇和 Bergamot 静态链接检查通过；4 个 sidecar 与主程序
  来源一致，release staging 为 68 个文件。保留未修改 `smoke_native.rs` 中
  `DELAY_NEXT_SETTLE_MS` 的一条 unused-static 警告；未抑制或夹带无关修复。
- 最后重新构建 debug 并校验 4 个 sidecar、主程序和 178 个 staging 文件；全部 exit 0。
  本检出最终保留普通 debug 产物，没有打包、Developer-ID 签名或安装载体。

## 原生证据的适用范围

channel 的传输、grant 发行/撤销含义、readiness、输入与锁顺序未改变；卡片 DOM、CSS、
事件行为与原 painter 相同。因此沿用验证索引所列 Slack/Pi/re-entry 证据的**原范围**，
不把新文件位置或新单测当作真实客户端、WKWebView、签名安装、Local Network 或 updater 认证。
本轮未启动 GUI、Mac mini、生产应用、真实账号、用户 Agent 配置，也没有 Developer-ID 签名、公证或发布。
真实 WebKit 卡片尺寸、键盘 focus 与点击的既有 smoke 本轮未重跑。

剩余债务：Board 仍拥有 poll/lifecycle/其它视图；其它授权来源仍消费原 JSON；
shared build output 仍需 provenance 检查。这里只抽一个职责并固定一条真实授权消费边界，
不扩到 settings 拆分、构建系统重写、通用插件或上游能力修补。

本轮没有执行远端 CI、Rust llvm-cov、macOS 14 兼容载体或签名安装；本地门禁不代表
整套 CI 多 runner 工作流已经通过。没有修改依赖，因此未在本轮重新运行 RustSec 网络审计。

## 完成条件与停止

| 要求 | 本轮证据 |
| --- | --- |
| 关键授权缺口 | 独立字段变异、明确缺省值、tick 事务与两个 late-revocation 窗口；81 项 channel 相关测试和完整 workspace |
| 实际 typed 消费路径 | `admit` → `documents::frozen_channel_head` → typed `frozen_matches` |
| 一个 Board 职责 | `board-card.js` 完整卡片展示/手势路由；动作返回原 owner；DOM 行为与静态边界测试 |
| 适用本地门禁 | `results.json` 的退出码、计数、日志及源码哈希；无测试断言放宽或新 ignored |
| 契约和证据 | documents/channel first-send/thread/Board/new renderer 头部、CLAUDE ownership 与 verification-status 同步 |
| 无扩权或新增产品功能 | storage/delivery/authority/select/persistence/lifecycle/依赖与 coverage gate 无 diff；没有 schema/IPC 或发布操作 |
| 最终范围审查 | 修改限定上述实现、测试和文档；无 settings/构建重写，没有提交或推送 |

达成这些条件后停止；未主动开展下一轮治理。
