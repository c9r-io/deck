# 关闭保护一致、排队行精确查询/取消、GUI 重启后的旧卡、API 错误结束（2026-11-28）

代码：main `3e3daccd`（关闭/队列/取消/接收状态/恢复事实）+ 合并 `d076987c`（StopFailure）与 `8028006d`（`deck_quota_snapshot`），以及其后的 smoke 提交。

## 完成时的预期

1. **升级或 GUI 重启后，活着但 Deck 看不到状态的工人卡不会被关掉。** `deck_worker_list` 显示 `liveness: alive`、`agent: unavailable / no-turn-seen`、`foreground.beforeDeckStarted: true`；episode 0、instance、gone 三种关闭都被拒（`OBSERVATION_UNKNOWN` / `SESSION_PRESENT`），提示不再建议换模式。等它下一回合被观察到再关，或交给用户。
2. **session 已不存在的旧卡可以清理。** `liveness: gone`、`instance: null` 时，用 `close_mode: gone` 加 `last_line`（列表给出）关闭，不需要升级前的 token；查询失败（`liveness: unknown`）不能清理；session 重新出现时拒绝。
3. **排队的行不会从视图里消失。** 任何还在队列里的行（queued / sending / uncertain）都列在 `sends`、计入 `activeLines`，不受最近 8 条历史限制；`deck_operation_get` 按 operation id 给出 `delivery`。
4. **改发插话前能精确取消旧行。** 查原 operation → `deck_worker_cancel` 得到 canceled → 用新 request id 发 interject → 查新 operation 的 delivery。已送达、发送中、结果不明、被用户编辑的行不会被取消或重发。
5. **报告接收状态不再混淆。** `reports.receiptState`：none / unowned / unknown（从未跟踪，不算已收到）/ pending / acknowledged；卡片标签显示「接收状态未知」或「已接收」。
6. **API 错误结束的 Claude 回合不再显示 working。** 卡片显示回合结束 + API 错误种类，MCP `agent.failure`；错误正文不进回复/摘要，不生成报告，不触发重试。
7. **没有活卡也能读额度。** 一次性勾选「读取账户额度摘要」后，`deck_quota_snapshot` 返回 Deck 已观察到的账号与 7 天窗口、观察时刻和新鲜度。

仍然做不到：没有对 Deck 看不到状态的活 Agent 的强制关闭；Codex / agy 没有正规的「回合因错误结束」事件；Claude/agy 的额度在 Deck 重启后要等下一次 statusLine；账号 ID 只在一次运行内稳定。

## 实测

- `air2-smoke-r4-*`：两阶段 PASS（见 `result.json` 的 `verdict-phase1/2`，客户端证据 `evidence/recovery/*.json`）。
- `air2-smoke-sf2-*`：Deck 卡片 pane 里真实 `claude -p` 被回环 400 结束 → app.log `turn-done failed=unknown`。r4 内的同一步骤没有结果：假 API 尚未监听时 Claude 第一次连接被拒，进入长时间重试；sf2 在 runner 里等假 API 回应后再输入。
- 作废的尝试（未保留）：r1–r3 复制自第二轮的 runner 把 `BIN` 写死为旧构建目录，跑的是默认 `run` 页面；已改为本 checkout 的 `target/debug/deck-app`。

## 复跑

`python3 run_e2e.py --host air2 --mode worker-recovery --label rN --timeout 2700`（约 25 分钟，需 air2 上 Claude Code 已登录；`--sf-only` 只跑 StopFailure 一步，约 3 分钟）。runner 只停自己登记的进程，结束后把运行根目录移入 air2 的废纸篓。
