# 插话绑定到 Agent 实例（2026-11-28）

`deck_worker_interject` / `deck_worker_send` 新增 `bind_mode: instance`，与原来的 episode 绑定并存。契约见 `mcp/workers.rs` 头部、`scheduler/select.rs`「Instance-bound worker lines」以及 `docs/mcp.md`「Binding a line to the instance」。

## 完成时的预期

1. **报告唤醒后直接调整同一个工人。** 编排者用唤醒结果里的 `instance` 和 `lastLine`，以 `bind_mode: instance` 插话。工人随后自己 Stop 不会让它被拒，不需要重读卡片再发。
2. **依赖某个快照的指令仍然严格。** 默认的 episode 绑定行为不变。被拒时返回 `queued: false`、`afterEpisode`、`currentEpisode`、`currentState`，明确这次什么都没排队。
3. **换了对象就不会误投。** 前台换成另一个程序、回到 shell 或 session 被重建时，入队前返回 `INSTANCE_CHANGED`。入队后这一行停在 `instance-gone`，自动发送、Send now、retry 和恢复入口都不会把它送给新对象，也不会为它重启 session。
4. **两个基于同一 lastLine 的请求只成功一个**，另一个返回 `LAST_LINE_MOVED` 和当前的 lastLine。

仍然做不到：没有隐式改投到新实例的功能，换实例必须由编排者重新决定；PTY 检查和输入之间不是对进程存活的原子锁，仍有极小的时间窗；Codex 子 Agent 的 hook 归属没有实测。

## 现场：65→87 是什么

生产 `~/.deck/app.log`（Deck 0.8.46，只含无内容的行）中 Codex 工人卡 `sess-9f244` 的时序：

| 时刻（epoch） | 事件 |
|---|---|
| 1795862544 | 本卡 `codex turn-done` e=62 |
| 1795862566 | `deck_worker_interject` 被拒 `EPISODE_MOVED`（该行未写卡片） |
| 1795862572 | 本卡一行送达；本卡 `codex working` e=65 |
| 1795862573–862875 | 其他卡（sess-4b563、5da4b、25de5、491c5、8874c）依次取走 e=66…86；**本卡没有任何 Signal** |
| 1795862882 | 本卡 `codex turn-done` e=87（本卡在 65 之后唯一一个被接受的事件） |
| 1795862892 | `deck_worker_interject` 被拒 `EPISODE_MOVED` |
| 1795862897 | 本卡一行送达（重读后再发），`codex working` e=89 |
| 1795863051 → 863059 → 863064 | 同样模式：本卡 turn-done e=98 → 被拒 → 5 秒后送达 |

结论：

- 65→87 只是**一次** `turn-done`。episode 是 Deck 进程内所有卡共用的观察编号，中间 21 个号被其他卡用掉了。
- 本卡没有重复、乱序或额外的 Signal。在这个时间窗里，同 pane 的 Codex 子 Agent 事件、async 迟到事件、Stop continuation 都没有出现，所以这次事故可以排除这几项。
- 原因：工人报告之后自己的 Stop 推进了 episode，而编排者是带着唤醒时的 episode 来插话的。插话被拒时什么都没入队，编排者重读后就成功了。
- 待证：旧日志里被拒的那一行不写目标卡，也不写 expected/current，这三次拒绝属于 9f244 是按时间推断的。本轮在拒绝日志里补了 `s=<卡> code state e`。报告写入本身也不进 app.log，所以「报告 → Stop」的先后只在下面 air2 的真实运行里实测过。
- 子 Agent：Codex 0.172 的 hook 载荷可以带 `agent_id`/`agent_type`（见 `docs/codex-signal-certification-20261008.md`），但多 Agent 的情形**没有实测**，也没有为它改 Signal。

## 隔离真实链路（air2）

WK 模式 `worker-bind`，air2 上本 checkout 的 debug 载体，独立数据目录和 tmux socket。工人是 Codex 0.169.0 `--no-daemon`，跑在运行自己的 CODEX_HOME 里：只有 Deck 的四个 Signal hook，没有子 Agent 或插件配置。客户端是 `scripts/mcp-bind-e2e.py`，在 Orch I 卡的 pane 里运行，通过真实的 deck-mcp adapter 调用。

| 运行 | 页面 | 客户端 | 说明 |
|---|---|---|---|
| b1（`air2-smoke-b1-*`） | PASS 5/5 | 证据丢失 | 页面在工人关闭后立刻关掉 Orch I，客户端写证据前就被结束了；app.log 时序与 b2 相同。已修：客户端每步保存，页面等 30 s 再关 |
| b2（`air2-smoke-b2-*`） | PASS 5/5 | **PASS 13/13** | `evidence/bind/bind-e2e-*.json` |

b2 实测（秒数从客户端启动算起）：

- tools/list 共 42 个工具，`deck_worker_interject` 和 `deck_worker_send` 的 `bind_mode` 枚举都是 `[episode, instance]`。
- 30.6 s：工人在自己的 shell 里调用 `deck_worker_report`，`deck_worker_wait until: report` 返回 `reason: report`。此时 episode 1，状态 `working`。
- 52.7 s：工人在报告之后自己 Stop（episode 2，instance 没变）。用 episode 1 发的插话被拒：`EPISODE_MOVED`，`queued: false`，`afterEpisode: 1`，`currentEpisode: 2`，`currentState: turn-done`。这就是现场症状，在真实 Codex 上复现了。
- 同一秒：用唤醒时的 instance 和 lastLine 发的插话被接受（committed），中间没有再读卡片；同一个 request_id 重放得到同一个 operation。
- 93.6 s：`deck_operation_get` 对这一行给出 `delivered`，距调用 40.9 s。等待原因是 Deck 对同一张卡两行之间保留的 60 s 间隔，不是 quiet 时间。96.6 s 工人按这一行建出了文件；被拒那一行要建的文件不存在。
- 96.7 s：再用旧的 lastLine 发一行，被拒 `LAST_LINE_MOVED`，`currentLastLine` 是刚才那一行。
- 99.8 s：`close_mode: instance` 关闭工人（committed）。
- app.log（`result.json` 的 `signalTimeline`）按顺序记下 `codex turn-done e=2` → `deck_worker_interject refused s=… code=EPISODE_MOVED state=turn-done e=2` → `sent … mode chain` → `refused … LAST_LINE_MOVED`。
- 清理：runner 登记的进程和 tmux server 都已停止，没有遗留；复制的 auth.json 已删除，运行根目录移进了 air2 的废纸篓。运行的 MCP 凭据文件没有进仓库。

没有在真实 Agent 上验证的（只有单元测试）：

- 排队以后换实例：前台换代、回到 shell、重建 session/pane、re-entry。单元测试覆盖了 `instance-gone`、粘贴前拒绝且不重试、不为它启动 session、逐字段比对。
- paste 和 Enter 之间换实例：Enter 被拒，这一行标为 uncertain。代码路径与已有的 needs-input Enter 拒绝相同，没有单独的真实测试。
- 同一 lastLine 的并发：单元测试 8 线程，同一张卡只有一个成功，不同卡互不影响。
- 重启后的恢复：queue.json v15，旧版本读到会拒绝而不是误发。

## 复跑

`python3 run_e2e.py --host air2 --mode worker-bind --label bN --timeout 2400`。需要 air2 上 Codex 已登录：runner 复制 `~/.codex/auth.json` 到运行自己的 CODEX_HOME，结束后删除。runner 只停止自己登记过的进程，结束后把运行根目录移进废纸篓。
