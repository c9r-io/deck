# 编排者 MCP：报告长等待、结构化报告、lane epoch、AGY 输入治理（2026-11-28）

代码：`d7ef0c99`（AGY 调度门禁）、`8f46e286`（MCP 等待/报告/lane）。基线 `0ce71dd8`，运行中的正式 Deck 为 0.8.44，本轮未替换。
环境：本机 MacBook Air M4，macOS（Darwin 27.2.0），Claude Code 2.1.332、Codex 0.172.0、agy 1.5.18。

## 门禁（本机，最终代码）

| 项 | 结果 |
|---|---|
| `cargo fmt`、`cargo clippy --workspace --all-targets --all-features -D warnings` | 通过 |
| CI 同款 llvm-cov（串行、隔离 HOME、空 TMPDIR） | exit 0；deck-app 1554 通过 0 失败；全部集成测试通过（signal_census 9 等）；行 85.04%、函数 82.57%；`check-test-tmpdir` 空 |
| deck-mcp（适配器单元 2 + stdio 黑盒 8） | 通过 |
| `scripts/ui-tests` | 1057/1057（第一次与 cargo 并行时有 1 项失败，单独重跑未复现） |
| `check-js-syntax.py`、`check.mjs` | 通过 |

新增测试：`mcp::admit::tests` 3 项；`independent_worker_requests_of_one_lane_go_at_once_under_lane_epoch`（序号连锁、ahead/behind、三线程并发 lane_epoch 其中一项因接管被拒、精确重放、冲突、记录裁剪后墓碑回答且不再入账）；`a_worker_report_carries_structured_claims_and_ends_a_report_wait`（字段校验、键序无关去重、同文异 SHA 成新报告、request_id、in_reply_to 归属、无读回复权限时 list/wait 不泄露、等待中报告到达 <5 s 返回、关卡返回 gone）；`an_agy_line_waits_while_agy_works_and_after_a_lost_word`；适配器 `a_long_wait_is_one_tool_call_over_bounded_deck_calls`、`a_codex_wait_is_capped_and_a_cancelled_wait_ends_at_once`、`a_worker_request_names_exactly_one_binding_before_connecting`。

## 真实 MCP 客户端（Claude Code 2.1.332，headless `-p`）

步骤：本仓库 debug 版 `deck-mcp`，合成凭据经 `--credential-fd`，`--socket` 指向临时目录中的模拟 Deck（Python，每次按 `wait_ms` 阻塞，到点返回 report）；`claude -p --strict-mcp-config --settings '{"disableAllHooks":true}'`，只允许这一个工具，cwd 为临时目录。不连接真实 Deck，不读 Keychain。

| 运行 | 请求 | 预期 | 实际 |
|---|---|---|---|
| r1 | `wait_ms` 120000，报告在 45 s | 一次工具调用，>30 s 后返回 report | exit 0，57 s；`reason=report`、`waitedMs=45006`、`deckCalls=3`；每段 `wait_ms=20000`，游标恒为 2 |
| r25 | `wait_ms` 1500000，报告在 1470 s | 接近上限仍为一次调用 | exit 0，1481 s；`reason=report`、`waitedMs=1469995`、`deckCalls=74`；游标恒为 2 |

清理：模拟进程已结束（`pgrep` 无残留）；临时目录在 `/private/tmp/claude-501/dk/rc`。副作用：两次 headless 会话在 Claude 配置目录下留有会话记录（按临时 cwd 归档）。

## 生产账本与日志（只读，计数）

- `~/.deck/mcp.json` 保留的 worker-send 窗口 1795782985–1795809699 覆盖 6 张 agy 工人卡全生命周期；这 6 张卡只有 start 与 close，没有任何 line 被接受。
- `app.log` 中 9 个 agy 会话只收到首行（`mode at`），没有 `mode chain` 投递。
- 审计 `request-denied`：全期 STALE_REQUEST 39、EPISODE_MOVED 54、RATE_LIMITED 40、CAPACITY_EXCEEDED 14；agy 卡存活期间 STALE_REQUEST 2、EPISODE_MOVED 1。审计不带工具与卡片，无法归属 → 原始事件**未唯一定位**；本轮起被拒的 worker 请求写一行 `[mcp] refused <tool> code=<CODE> relation=<…>`。

## UNRUN

- 交互式 Claude Code 超过 2 分钟自动转后台后的行为、Esc 取消的真实客户端路径（仅适配器黑盒测试）。
- Codex 真实客户端（50 s 上限仅黑盒测试；未在本机跑 `codex exec`，避免其 hooks 向正在使用的 Deck 发送事件）；Pi 客户端。
- 真实 Deck 应用内的端到端（WK smoke、air2）：lane_epoch 并发、报告等待、agy `agy-working`。
- 真实 agy 的权限框场景（已显示、粘贴与 Enter 之间弹出、Esc、状态过期）在新门禁下的复测。
- 额度观察：未实现（见设计文档 E，待决）。
