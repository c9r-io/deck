# 可直接使用的工具示例（round 2）

所有调用都从编排者自己的 Deck 卡片里发出（Deck 从内核读出调用来自哪张卡）。

## 1. 从外置 worktree 直接开卡

前提：用户已在 Permissions… 勾选「自动允许已授权项目的关联工作树」。`deck_capabilities.autoLinkedWorktrees`
为 `{"enabled": true, "projects": [{"projectId": …, "root": …, "anchored": true}, …]}`。

```json
{"tool": "deck_worker_start", "arguments": {
  "request_id": "start-dev77-1", "lane_epoch": 3, "type": "claude",
  "dir": "/Volumes/YottaUnity/seventh-dev77",
  "text": "…任务…完成后调用 deck_worker_report（fields.status、branch、完整 commit、report_path）"}}
```

接受时 `result.cwdGrant` 为 `auto-worktree`；`deck_operation_get` 的 `result.launch` 记录实际启动前的最终准入。失败时：

```json
{"ok": false, "error": {"code": "PATH_NOT_ACCESSIBLE", "reason": "worktree-unverified",
  "detail": "separate-repository", "message": "…", "nextAction": "…"}}
```

`reason` 为 `volume-unavailable` 表示外置盘未挂；`PATH_AMBIGUOUS` 时带上 `project_id` 重试同一 dir；
最终准入拒绝时 operation 为 `rejected`、`code: launch-refused`、`result.launch.reason` 说明原因。

## 2. 工人提交 handoff

```json
{"tool": "deck_worker_report", "arguments": {
  "text": "资源整理完成，剩余的场景接线交给你安排。详见 report.md",
  "fields": {"status": "handoff", "branch": "dev77-assets", "report_path": "report.md", "task_ref": "DEV-77"},
  "request_id": "dev77-report-1"}}
```

## 3. 长等待取得正文 → 确认该序号

```json
{"tool": "deck_worker_wait", "arguments": {"cards": [{"card_id": "W123", "episode": 0, "reports": 0}],
  "until": "report", "include_reports": true, "wait_ms": 1500000}}
```

从返回的 `reportsInline.cards[].reports[]` 取 `sequence`（不是 `workers[].reports.latest`）：

```json
{"tool": "deck_worker_reports_ack", "arguments": {"card_id": "W123", "sequence": 1}}
```

→ `{"ok": true, "acknowledged": 1, "latest": 1, "awaitingAck": false, …}`。响应丢失就原样再发一次。

## 4. inline 被 omitted 后补取再确认

`reportsInline.cards` 里出现 `{"cardId": "W123", "omitted": 2}` 表示正文没返回：

```json
{"tool": "deck_worker_reports", "arguments": {"card_id": "W123", "after": 1}}
```

读到 `reports[]` 后确认其中最大的 `sequence`；`firstKept` 大于你想要的序号说明更早的已不在保留窗口。

## 5. 按实例关闭，并等最终结果

从同一次 wait/list 的视图取 `instance` 与 `lastLine`：

```json
{"tool": "deck_worker_close", "arguments": {
  "request_id": "close-W123-1", "lane_epoch": 3, "card_id": "W123",
  "close_mode": "instance", "instance": "9c1f…(32 hex)", "last_line": "op_ab12…"}}
```

`lastLine` 为 null 时不要传 `last_line`。返回 `state: accepted` 只表示已接受：

```json
{"tool": "deck_operation_get", "arguments": {"operation_id": "op_…"}}
```

`committed` = 卡已关闭；`rejected` 时看 `code` 与 `result.refusal`（`stage`、`observed`、`nextAction`）。

## 6. 精确重放与新决定

- 网络丢了响应：**原样**重发第 5 步（同一 `request_id`、同样参数）→ 返回原 operation（或其最终状态）。
- 被拒后要改条件（例如换成 episode 模式，或者用新的 instance）：这是新决定，用**新的** `request_id`。用旧 id 配新参数会得到 `REQUEST_ID_CONFLICT`。
