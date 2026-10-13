# 技术债治理：验证同步、hooks 配置职责与验证索引

2026-10-13；基线 `84b7783c51198e1a89e1929380b2a5845cc2c77b`（0.8.1），开始时工作树干净。
本轮结果绑定 `results.json` 的工作树文件 SHA-256，不把基线 CI 的通过算作本轮 CI。
开发环境为 macOS 27.0.1 arm64、Rust/Cargo 1.97.1、Node 24.19.0（CI 的 Node 为 22）。
这是执行记录，不是新增发布或所有原生场景的认证。

## 1. 测试同步与诊断

### tmux

历史 [CI 38281532520](https://github.com/c9r-io/deck/actions/runs/38281532520)
在 `scheduler_context_identity_process_and_hookless_compatibility_contract::query_client_plus_pane`
等待 shell prompt 时失败。日志没有失败的具体建会话命令；原来的 `Server::run`
丢弃退出码与 stderr。先仅增加退出码检查，在本机重复 100 轮、300 个拓扑用例全部通过，
因此没有证据把那次历史失败唯一归因于停止／重建竞态。

本轮改变的是能够从代码证明的薄弱处：

- 每条预期成功的调用检查退出码并报告具体 argv 和 stderr，不把命令失败伪装成空输出。
- 两处在同一 socket 上替换 fixture server 的用例先登记 `kqueue EVFILT_PROC / NOTE_EXIT`，
  再发送原有的 `kill-server`，观察该执行实例退出后才创建新 server。
  没有按 pid 发送信号，也没有把列表缺席当作退出事件。只作用于测试自己的 server。
- 原有 readiness 的 500 次检查／10ms 间隔预算保留；没有增加全测试重试、放宽产品超时、
  改变 CI 串行策略或忽略失败。
- 第一次完整合约检查 exit 101：三个拓扑的 dead-target 测试本来就要得到失败，
  另外两处 fixture 在没有进入 copy mode 时发送了 `cancel`。现在前者明确断言失败及其 stderr，
  并保留“后续命令没有运行”的原断言；后者断言起始 mode 为 0 后进入 copy mode。
  修正后完整 111 项通过；加入退出事件同步后的 100 轮、300 个拓扑用例也通过。

历史 CI 超时仍为**待观测**。新的退出同步消除了未经观察就重建的窗口，
但没有历史日志就不能声称已证明根治。下一次正常提交的 CI 是下一条证据；
再失败时先使用新增诊断区分建会话失败和 shell 就绪问题，不重跑直到绿。

### Pi

`r01_no_child_of_the_observer_writes_anything_for_it` 的最后一组原来睡 300ms，
然后假定父进程已经连接并写完心跳。现在 fixture 的 `@sent:<file>` 在下一条完整消息
首次写出后、读取回答前创建标记，测试等待它；普通消息收到 `again` 后的重试语义不变。

第一次用 `@open / @line / @read` 分解普通消息的尝试 exit 101：它丢失了普通消息的
nonce 重试，日志只有 `again` 而没有预期的 ack 3。该尝试已替换为上述写入标记，
原 ack、拒绝原因与不污染其它 pane 的断言保留。最终 41 项 transport 测试通过。

其余睡眠不是一概删除：nonce／answer／staleness 到期用例仍检查真正的时间约束；
已有的真实进程身份用例使用 `UNHURRIED`，定时失效有独立测试。
没有修改 observer、transport 的产品时间参数或归属证明协议。

### 剪贴板 smoke

`smoke_clipboard_metrics` 使用固定 `/usr/bin/pbpaste` 和 `LC_ALL=en_US.UTF-8`。
LC_ALL 优先于继承的 LC_CTYPE/LANG，因此启动环境缺失 locale 或为 C locale 都不决定读取编码。
新增测试检查实际 child command 的覆盖项；原 smoke 禁用保护与 EDR 封闭进程清单继续通过。
本轮没有读取／改写用户剪贴板。真实窗口的非 ASCII 拷贝仍留待下一次 WK smoke，不以命令检查冒充。

## 2. hooks 配置职责

- `agent_status/hooks.rs` 拥有配置编辑、helper 路径、安装状态、设置命令、迁移和静态 adapter 声明；
  文件头列出精确职责与限制。
- `agent_status/hooks/tests.rs` 持有对应配置测试；父模块保留 live Signal、身份判定、状态与 listener 测试。
- 父模块重新导出现有 Rust／Tauri 路径。未增加 IPC、schema、权限或第三方依赖；
  Pi 仍使用同一个保留原始 JSON 字节的编辑器和 atomic writer。
- 搬出的生产主体与原主体逐字比对完全一致；剩余 Signal／listener 主体及测试也同样比对。
  所有原测试／辅助函数仍在，raw string fixture 的字节未变。子进程测试的精确名称随模块位置更新。
- Signal census 的三类 literal 合计次数保持原值，只将 hook spec 的计数转到新路径；
  数据目录例外和 release helper marker 的源码检查同样跟随真实 owner，没有扩大 allowlist。
- `CLAUDE.md`、第一方扩展说明及相邻模块的 ownership 指向同步更新。

## 3. 验证与适用范围

最终命令、退出码、计数和源码哈希见 [results.json](results.json)。

| 检查 | 最终结果 |
| --- | --- |
| Rust workspace，all-targets / all-features，串行 | exit 0；1,519 passed，0 failed，13 ignored（原有需专门环境／工具调用的用例） |
| 最终 hooks / Signal 模块与 tmux 合约复核 | exit 0；65 passed / 3 ignored，以及 111 passed |
| cargo fmt / clippy `-D warnings` | exit 0 |
| JS 语法、UI tests / coverage、模块标识符检查 | exit 0；781 passed；行 94.29%、分支 89.49%、函数 87.07% |
| Python release / registry / verification tooling | exit 0；181 tests，OK |
| release 构建、EDR 二进制词汇、Bergamot 链接检查 | exit 0；release 有未改动模块 `smoke_native.rs` 的一条 unused-static 警告，未抑制 |
| release helper 标记及 sidecar 来源 | PASS；可执行文件仍含 helper 名与专用标记；4 个 sidecar 来源检查通过 |
| 最终 debug 构建与 sidecar 来源 | exit 0；已恢复本检出的普通 debug 产物 |

全 workspace 运行后，仅补齐了注释、恢复原 hook fixture 字符串的缩进、保留 tmux 原有轮询预算并改善报错。
最终 hooks 与完整 tmux 合约在这些调整后重跑，最终 clippy 与 release/debug 构建同样在最终代码上运行。

只记录实际运行的检查；coverage gate、签名发布、真实客户端认证和 GUI 验证不由普通单测替代。
原始运行日志留在执行机 `/tmp/deck-debt-*.log`，是本机临时材料，不是其它检出的依赖；
可独立理解的结论与失败经过保留在本目录。

[当前验证索引](../../verification-status.md) 将本轮结果与 Pi、Codex、re-entry、Slack、
构建及实验清理的历史证据分开，列出环境、适用范围、未覆盖项和重验触发条件。
没有发布、推送、修改用户 Agent 配置，或启动 Mac mini／正式应用实验。

## 4. 本轮停止边界

按顺序完成验证基础、一个核心职责的抽取、验证索引。Board／documents 是后续候选，
本轮没有同时拆它们；上游能力缺口仍保留原来的 BLOCKED。
不以所有历史 UNRUN 清零、文件变短或一次 CI 变绿作为验收标准。
