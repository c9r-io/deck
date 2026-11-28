# 退出超时的 Claude 卡：late exit（2026-11-28）

起因：本机 2026-11-28 两次升级重启（01:56、11:22 JST），重入失败的 5 个 Claude 会话都有后台任务在跑。Claude Code 2.1.332 遇到这种情况，收到 Ctrl+D（以及 `/exit`）时会弹出它自己的确认框「Background work is running」，不会自行退出；Deck 不在 Agent 的对话框里按键。等待 10 秒后，这张卡被判 `exit-timeout`，不再重入。调查报告在仓库之外：`~/c9r-io/deck-reentry-bg-20261128/report.md`。

## 修复

- 重启的退出等待结束后，对已勾选、旧客户端仍在运行的 Claude 条目，Deck 向该客户端（就是冻结时那一代进程）发送 SIGHUP，此时它的 shell 和 tmux 服务都还在（`Plan::end_late_clients`，`procinfo::hang_up`，专用时长 3 秒；准备阶段 23 秒，总时长 28 秒）。客户端退出时自己上报 `SessionEnd`（reason `other`），用它确定最终会话（`claude::final_after_hangup`）。
- 只有当这张卡的崩溃恢复记录（同一代客户端）写的是同一个会话，并且在开 shell 之前完成认领（核对后删除并写盘），才会启动（`recovery::claim_at`）。勾选、Policy、携带的启动参数、配置目录和确认流程都与正常退出相同；每张卡仍只有一次启动机会，结果按 `Route::{Exit, LateExit}` 区分。
- 某个已武装 lease 的回执被丢弃（队列满）、核验被拒或迟到，或者在 lease 武装期间有连接没读到内容就被丢掉，这个 lease 的记录记为不完整（`lifecycle::Event::Lost`），不再据此认定任何最终会话。
- 记录器在会话结束原因为 clear/resume 时删除对应记录；已认领的那一代客户端，迟到的事件不会把记录写回去；崩溃恢复只为 `consume` 实际删掉的记录启动。

## 原生事实（air2，Claude Code 2.1.332）

`probe-hangup-hooks.jsonl`：用自己的 `--settings` 钩子做的探针，会话 id 已脱敏。五种状态（空闲；确认框打开；前台工具运行中；`/clear` 之后；有后台任务但没弹框）下，只向客户端本身发 SIGHUP、shell 不动，结果一致：约 0.2 秒后 `SessionEnd reason=other`，携带的是**当前**会话 id（`/clear` 之后就是新 id）；钩子的父进程就是客户端；约 0.7 秒后客户端结束，前台和后台的工具进程也随之结束；没有遗留任何 Claude 进程。对中途打断的会话执行 `--resume`，界面显示「Interrupted · What should Claude do instead?」并停下等待输入。

## 真实 Deck 全链路（air2，debug 载体，隔离 home + 私有钥匙串，真人登录）

模式 `reentry-late`（`app/ui/test/reentry-late-smoke.mjs`，复用 relive 的 harness 和根目录），跑在 `/private/tmp/deck-relive-<12 hex>` 下，由真实对话框驱动。四张卡：A 有后台 shell；B 空闲；C 前台工具运行中；E 后台 shell 和前台工具都在跑。一次重启，四张卡全部勾选，Policy A。

| 次 | 结果 | 说明 |
|---|---|---|
| r1 | FAIL（判定前提有误） | 只有三张卡。C（只有前台工具）收到 Ctrl+D 后自己退出了，说明**仅有进行中的一轮不会弹确认框**；我原先预期它走 late exit，这个预期错了。A 走 late exit，结果为 confirmed。runner 另有一处缺陷：photo 里 `ps` 命令行本身含有字面 `## sessions`，导致进程表被截断（已修）。 |
| r2 | 检查全过，verdict 因 manifest 期望值少写 1 而 FAIL | 加入 E 后，manifest 里 `late-request` 的期望仍是 3 个勾选，实际勾选 4 个。修正期望值后，用同一份 app.log 离线重判为 11/11。 |
| r3 | **PASS** | 用最终文件干净重跑：smoke-verdict 11/11，runner 0 个问题（`run-r3.log`、`app-log-r3.txt`）。 |

r3 的关键行：`exit-timeout remaining=2`；`late-exit sent=2`，随后 `late-exit done sent=2 gone=2 elapsed_ms=678`；`sealed final=4 of=4 late=2`；四条 entry 都是 `outcome=confirmed`，其中两条 `route=late-exit`、两条 `route=exit`；`complete replaced=true entries=4 confirmed=4`。页面确认 A 带 late-exit 那一行，B 不带；`late-routes a=2` 表示 E 也走了 late exit，C 走的是它自己的退出。结果以列表显示，没有合并成提示条。重启后：C 和 E 显示 Interrupted，停在输入框等待，5 秒后仍未自己继续；A 显示后台 shell 没跑完。进程表确认：四张卡各自只有一个重入的 `claude`（argv[0] 为 `claude`），会话就是重启前最新的那一个；旧的后台和前台工具进程都已结束；旧客户端的恢复记录都不在了（late exit 的已认领，自行退出的由其退出回执删除），新客户端写了自己的记录。

清理：registry cleanup 退出码 0；登出；删除私有钥匙串；移除根目录；本机自己的 hooks 文件 digest、`~/Applications`、正式 tmux 服务和默认钥匙串都与开始前一致。

## 门禁（commit `8e8daf9b`）

fmt、CI 形式的 clippy 均通过。CI 同款 `cargo llvm-cov --workspace --all-targets --test-threads=1` 退出码 0：1896 个测试通过、0 个失败，行覆盖 85.14%，函数覆盖 82.77%，TMPDIR 和 tmux socket 目录都没有遗留。`scripts/ui-tests` 1065/1065，JS 语法检查、`check.mjs`、extension catalog 均通过。smoke 模式的修正（manifest、页面脚本、诊断名）在后续 commit 中，复检了 clippy、diagnostics 测试和 ui-tests。

## 没有验证的部分

- 后台事项中实测过的只有后台 shell 一种；Monitor、子代理、队友、MCP 任务按代码判断会走同一个确认框，但没有实测。
- 确认框第 2 项「Move to background and exit」没有测。
- 发布包、真人 home 及其全局钩子文件、Codex（设计上不做 hang up）。
- 前台工具运行时的 Ctrl+D 是否总是直接退出：r1、r2、r3 中 C 都是这样，但这是 Claude 自己的行为，不是 Deck 的保证。
