# Pi v0.8.0 Follow-up 03：残余风险 1 的有界测量（执行者证据）

对应 Follow-up 03 第 3 件（`docs/design/pi-v0.8.0/follow-up-03.md`，负责人本地文档，不在版本控制内）。本文件由执行 Agent 撰写，是提交给负责人复核的材料，不是验收结论：下面的结论是执行者自评。负责人的独立验收记录在 `docs/evidence/pi-followup-03-acceptance-20261012/`（负责人本地文档，不在版本控制内）。

测量在 2026-10-12、本机（macOS 27.0.1，arm64）进行，不运行 Deck，不改产品代码；当时仓库在 `dfec67c`。被测对象：

| 对象 | 身份 |
| --- | --- |
| Node 24.19.0（libuv 1.52.1） | 官方可执行文件的副本，SHA-256 `27db838bb204ef7c21df2931f5656e4c8fb32e6e947f363a402b49714d32b5b1` |
| Bun 1.3.14（修订 `0d9b296af33f2b851fcbf4df3e9ec89751734ba4`） | 官方 standalone Pi 1.1.0 内嵌的 runtime（该文件 SHA-256 `daea10db90f7806cd0ae5be89fcf819d47b5ed3df99b7c79c31186a1cd6a6501`），以 `BUN_BE_BUN=1` 当作普通 Bun 运行 |
| Pi 1.1.0（`@earendil-works/pi-coding-agent`） | npm 包的 `dist/bundle/cli.js` SHA-256 `e79626f2dd6f94aa45d30f3fa63cd84319a6eefcd150b353cfaf274366926774`，以及上面的 standalone |

## 结论

所问的两点都已证实；残余风险 1 要关闭所需的更强前提发现反例，没有修复，作为设计问题交回。

| 所问 | Node 24.19.0 | Bun 1.3.14 |
| --- | --- | --- |
| `net.createConnection({ path })` 的 `socket()` + `connect()` 在 JS 线程的一次原生调用内 | 已证实 | 已证实 |
| `child_process` / `Bun.spawn` 只由发起调用的 JS 线程创建子进程 | 已证实 | 已证实 |
| Pi 1.1.0 自己的工具执行 | 已证实（主线程） | 已证实（主线程） |
| 进程里没有别的线程会创建子进程 | 发现反例（Worker） | 发现反例（Worker、`Bun.openInEditor`） |
| 别的线程创建的子进程持有 JS 线程正在建的连接 | 未证实（1000 次未观察到） | 发现反例（9/1000） |

- **反例。** Bun 的 `Bun.openInEditor` 在自己的线程上 fork + exec（`src/cli/open.zig:320`、`:333`）。1000 次调用，1000 次 `fork()` 全部来自名为 “Open Editor” 的线程；其中 9 个子进程生来持有 JS 线程刚用 `net.createConnection` 建的连接，各写入一行，监听端收到 9 行（接下连接时内核给的 peer 是 Bun 进程，子进程写入后变为子进程）。原因是 macOS 没有 `SOCK_CLOEXEC`，close-on-exec 在 `socket()` 之后另行设置（uSockets `bsd.c:644` → `:367`）。两种 runtime 里 Worker 线程都能自己创建子进程。Pi 1.1.0 的 bundle 不调用 `Bun.openInEditor`，扩展可以。
- **方法。** 不注入、不调试：一个 Node-API addon（`measure/only.c`）在被测进程内用 `thread_suspend` 挂起其它全部线程，只留 JS 线程，再看连接与子进程是否仍然产生；`pthread_atfork` 记下每次 `fork()` 由哪条线程发出。
- **失败记录。** 挂起其它线程时 JS 线程偶尔停住，共 4 次尝试经 registry 停止、整次不计、另起进程补足（见“失败记录”）。
- **清理。** 实验根 `/private/tmp/deck-pi-r1-Vz99if`，`experiment cleanup` exit 0，登记的 214 个进程全部 gone（见“清理”）。

## 方法

- `measure/only.c`：`hold()` 对本进程的线程逐一 `thread_suspend`（调试器用的同一个 Mach 调用，只作用于自己的 task），`release()` 恢复。被挂起的线程不执行任何用户态指令，所以挂起期间内核为这个进程做的事，都是发起调用的那条线程要求的。
- 对照（`control`）：确实离开 JS 线程的工作（线程池里的 `crypto.pbkdf2`）在挂起期间不完成，恢复后 2–3 毫秒完成；同一期间 JS 线程自己的事件循环转了 28 次。Node、Bun 各 3/3。
- 连接（`connect`）：另一个进程里的监听端（`measure/listen.c`）被问到时以零超时回答“此刻有没有连接在等”，并给出内核的 peer pid。每一轮在挂起之下先问一次（应为没有）、调用 `net.createConnection({ path })`、调用一返回再问一次（应为有，且 peer 是本进程）。
- 子进程（`spawn`）：被创建的程序（`measure/kid.c`）写下自己的 pid、ppid 和生来持有的全部描述符。每一轮在挂起之下调用一个子进程 API，并在同一线程上等到子进程自己的报告出现。
- 一轮只在“整段挂起”时才算证明：前后线程集合相同、其它线程在两次读数时都处于挂起、且没有一条的程序计数器与栈指针变过。
- 谁在 fork：`watchForks()` 注册 `pthread_atfork` 的 prepare 回调，C 库在调用 `fork()` 的那条线程上、fork 之前运行它。它对 `posix_spawn` 不说话；`posix_spawn` 的调用者只由“其它线程都被挂起”排除得出。
- 没有使用的手段：`DYLD_INSERT_LIBRARIES`（EDR 的检测模式）、`lldb`（本机 `DevToolsSecurity` 为 disabled，会弹出授权对话框）、dtrace、`sample`。
- 运行方式（`measure/measure.py`）：每个被测进程和它的两个监听端都从实验根启动，先登记到根的 experiment registry（`scripts/edr_runtime.py`）再开始；环境只有根内的 HOME 与 TMPDIR、`PATH=/usr/bin:/bin`。`Bun.openInEditor` 的“编辑器”是按绝对路径给出的根内 `bin/kid`，机器上的任何编辑器都不会被启动。

## 依据

### 源码与二进制

- Node：`lib/net.js:1304` 的 `self._handle.connect(req, address)` 同步进入 `src/pipe_wrap.cc:230`，再进入 `uv_pipe_connect2`（`deps/uv/src/unix/pipe.c:254`），它在同一个 C 函数里先 `uv__socket`（`:289`）后 `connect`（`:305`）。子进程只经 `uv_spawn`：通常是 `posix_spawn` 加 `POSIX_SPAWN_CLOEXEC_DEFAULT`（`deps/uv/src/unix/process.c:503–505`、`:710`），要求 `uid`/`gid` 时改走 `fork()`（`:483–489`、`:841`）。
- Bun：`src/js/node/net.ts` 的 unix 连接经 `Listener.connectInner` 进入 uSockets 的 `internal_bsd_create_connect_socket_unix`（`packages/bun-usockets/src/bsd.c:1649–1663`），同一个 C 函数里先 `bsd_create_socket`（`:1650`）后 `bsd_do_connect_raw`（`:1658`）。子进程通常是 `posix_spawn` 加 `POSIX_SPAWN_CLOEXEC_DEFAULT`（`src/runtime/api/bun/process.zig:1374`）；带终端的 spawn、崩溃处理和 `Bun.openInEditor` 走 `fork()`。
- 报告引用的每个源文件的 SHA-256、两个源码包的来源与校验在 `sources.txt`。Bun 的源码与被测二进制的对应关系以二进制自报的版本与修订为准。
- 二进制里的调用点（`measure/callers.py` → `out/callers-node.json`、`out/callers-bun.json`、`out/callers-bun-context.json`）：Node 可执行文件里 `fork` 与 `posix_spawn` 只被 `uv_spawn` 调用，`uv_spawn` 只被 `ProcessWrap::Spawn`、`SyncProcessRunner::TryInitializeAndRunLoop`、`task_runner::ProcessRunner::Run` 调用。Bun（无符号）有 2 处 `posix_spawn`、3 处 `fork`、1 处 `posix_spawnp`，按邻近的字符串对应到常规 spawn 与 `process.execve`、带终端的 spawn 与 Zig 标准库的子进程（`openInEditor` 用的那个）与崩溃处理、libarchive。只统计直接调用。

### 测量（`out/s2-node.summary.json`、`out/s2-bun.summary.json`）

| 项目 | Node | Bun |
| --- | --- | --- |
| `connect`：挂起之前没有连接在等、调用返回时已在等、peer 是本进程、整段挂起 | 300/300 | 300/300（另有一次尝试被停止） |
| `spawn`：子进程在整段挂起之下产生、是本进程的子进程 | 7 个 API 各 40，280/280 | 9 个 API 各 40，356/360 |
| 同上：子进程生来只有标准三个描述符（进程自己同时留着一条连接） | 280/280 | 360/360 |
| `forkers`：普通 `spawnSync` 的 `fork()` 次数 | 0（5/5） | 0（5/5） |
| `forkers`：走 fork 的路径由谁 fork | 带 `uid`+`gid`：主线程（5/5） | 带终端的 `Bun.spawn`：主线程（5/5）；`Bun.openInEditor`：另一条线程 “Open Editor”（5/5） |
| `worker`：主线程休眠 119 毫秒期间，Worker 线程创建的子进程 | 100 | 100 |
| `race`：Worker 以常规方式造 1000 个子进程、主线程同时建连接，生来持有连接的子进程 | 0/1000（289,840 条连接） | 0/1000（218,178 条连接） |
| `race-fork`：Worker 走 fork 路径造 1000 个子进程，同上 | 0/1000（437,146 条连接） | – |
| `editor-race`：1000 次 `Bun.openInEditor`，生来持有连接并写入一行的子进程 | – | 9/1000（主线程其间建了 1,465 条连接） |

Bun 的 `spawn` 有 4 轮不计为证明（`execFile` 1、`exec` 2、`execSync` 1）：JS 线程在挂起期间自己新建了一条 JIT 线程，线程集合变了；这 4 轮的子进程同样在挂起之下产生。被挂起的线程在挂起期间被记到的处理器时间，用户态始终是 0，内核态至多 12,000 纳秒（Node 至多 8,000；是内核唤醒一条本已阻塞的线程的开销），程序计数器与栈指针不变。Node 的 `race-fork` 是“1000 次未观察到”，不是“不可能”。

### 真实 Pi（`out/pi-npm-2.pi.run.json`、`out/pi-standalone-2.pi.run.json`）

Pi 以 print 模式跑一次，由本地 fixture provider（`measure/r1-ext.ts`，不联网）让它调用一次自己的 shell 工具；扩展只在 runtime 创建子进程的那一次调用外面套上 `hold()` / `release()`。

| | npm 包（Node） | standalone（Bun） |
| --- | --- | --- |
| 创建子进程的调用 | `ChildProcess.prototype.spawn('/bin/bash', …)`，来自 Pi 的 `executeToolCallsParallel` | `Bun.spawn('/bin/bash', …)`，经 `node:child_process` |
| 所在线程 | 主线程（`isMainThread`，`threadId` 0） | 主线程 |
| 挂起 | 其它 12 条线程全部挂起，整段 | 其它 12 条线程全部挂起，整段 |
| 子进程 | 是 Pi 的子进程，生来只有标准三个描述符 | 同左 |
| 从扩展加载到 agent 结束，本进程里的 `fork()` 调用 | 无 | 无 |

对 npm 包的 bundle（`dist/bundle/chunks`，73 个文件）的普查：只通过 `node:child_process` 创建子进程；有两处 `new Worker`（codemode 沙箱、图片缩放），两个 worker 入口文件里都没有创建进程的 API；原生 addon `darwin-platform.node` 只导入剪贴板、AppKit、CoreGraphics 与 N-API；没有 `Bun.spawn`、`bun:ffi`、`openInEditor`、`node-pty`、`process.dlopen`。普查是只读的文本搜索与符号表查看，结果没有另存文件。

### 反例的最小构造

在一个按“重跑”一节建好的根里（每次换一个名字）：

```sh
python3 $R/src/measure.py $R one x1 bun R1_PHASE=forkers R1_COUNT=3
python3 $R/src/measure.py $R one x2 bun R1_PHASE=editor-race R1_COUNT=1000
```

这类子进程对 Deck 的四项证明意味着什么（是否冒充、代价多大）是设计问题，没有对 Deck 的 transport 实测，由负责人裁决。

## 失败记录

- 挂起其它线程时，JS 线程偶尔停住不动。共 4 次尝试因此经 registry 停止：最初两次不分块的长跑（各等到 240 秒的看门狗），以及最终系列里 Bun 的两次（`s2-bun-connect-1` 停在第 20 轮，`s2-bun-child-process-exec-1` 停在第 11 轮）。停住的尝试整次不计；运行器此后改为一项一个进程、每轮写进度、6 秒不动即停并为剩余次数另起进程。
- 这 4 次停住本身没有被观测。负责人验收之后我做了一次补测：给 addon 加一条不被挂起的见证线程，停住时读下每条线程的位置并放开其它线程。11,760 轮挂起里捕获 8 次（Bun 4、Node 4），每一次 JS 线程都停在一把锁上，锁的持有者是一条被挂起的线程（Bun：内存分配器 libpas 的后台回收线程；Node：V8 后台编译任务的工作线程），放开后 JS 线程自行继续。它是这种测量方法的副作用，不是被测调用在等别的线程。补测的程序、带函数名的调用栈与对照在 `stall/`（先看 `stall/README.md` 与 `stall/out/observed-stalls.txt`）；它的两个实验根的原始结果留在本地（见“留在本地的材料”）。
- 第一系列与最终系列之间程序改过（加了 `forkers`、`race-fork`、`editor-race`，子进程改为报告 socket 状态并写一行）。结论只依据最终系列（`s2-`）。

## 清理

- 实验根 `/private/tmp/deck-pi-r1-Vz99if`（保留）。`python3 scripts/edr_runtime.py experiment cleanup --root …` exit 0；它的输出在 `out/cleanup.log`（214 行，每个登记的进程一行，全部 gone），registry 本身在 `out/experiment-registry.json`；进程表里没有来自该根的进程。
- 被测进程自己创建的约 9,800 个毫秒级子进程，以及我直接运行的 3 条毫秒级命令，没有逐个登记。
- 根外没有写入：`~/.pi`、`~/.npm`、`~/.cache`、`~/.config` 在根创建之后没有新文件；用户自己的 `node` 与全局 npm 里的 Pi 包只被读取。
- 没有用 pid、`pkill` 或 `tmux -L` 结束任何进程；测试机没有使用。

## 限度与未运行

- 未运行：完整门禁、native 矩阵、真实窗口、测试机（本轮无行为改动）。
- 未运行：反例对 Deck 实际 transport 的效果。
- 未运行：Pi 的两个 Worker 功能（codemode、图片缩放）的实际运行；standalone 内嵌 bundle 的单独普查（普查的是同版本 npm 包）。
- 只在 macOS 27.0.1 arm64、这一台机器、这两个 runtime 版本与 Pi 1.1.0 上测过；真实 Pi 各只跑了一次 print 模式，不是 TUI。
- “一次原生调用”来自源码；线程身份来自“其它线程被挂起”的排除法加 `pthread_atfork`。

## 本目录的文件

`measure/`（测量程序，全部 TEST ONLY，只在实验根里运行）：

| 文件 | 作用 |
| --- | --- |
| `only.c` | Node-API addon（Node 与 Bun 都能加载）：`hold()` / `release()`、`threads()`（每条线程的名字、内核记的时间、挂起计数、程序计数器与栈指针）、`watchForks()` / `forks()`。 |
| `listen.c` | 另一个进程里的监听端。`probe` 模式回答“此刻有没有连接在等”与 peer pid；`all` 模式接下每条连接保留 20 毫秒，收到子进程写的 `kid <pid>` 时记一行。 |
| `kid.c` | 被创建的子进程：写下 pid、ppid、生来持有的全部描述符；对标准三个之外的 socket 说明其状态并写一行。 |
| `probe.cjs` | 在 Node 与 Bun 里原样运行的探针，一个进程跑一个项目（`R1_PHASE`）。 |
| `measure.py` | 运行器：登记、隔离环境、停滞检测与补跑；`series <node|bun> [TAG]` 跑全部项目。 |
| `pirun.py`、`r1-ext.ts` | 真实 Pi 的一次运行与它加载的扩展。 |
| `callers.py` | 只读：扫描 arm64 可执行文件里对 `fork` / `posix_spawn` / `socket` / `connect` 的直接调用点。 |
| `summarize.py` | 只读：把一次 series 的结果打印成一张表。 |

`out/`：

| 文件 | 内容 |
| --- | --- |
| `s2-node.summary.json`、`s2-bun.summary.json` | 最终系列每个项目的合计、每次尝试、被停止的尝试，以及所用程序与二进制的 SHA-256。 |
| `series-s2.log` | 最终系列的运行日志（每次尝试一行）。 |
| `pi-npm-2.pi.run.json`、`pi-standalone-2.pi.run.json` | 真实 Pi 的两次运行。 |
| `callers-node.json`、`callers-bun.json`、`callers-bun-context.json` | 二进制调用点普查。 |
| `experiment-registry.json`、`cleanup.log` | 实验根的 registry 与清理日志。 |

`sources.txt`：引用的源文件的 SHA-256 与两个源码包的来源。

本目录的文件是本地材料的逐字节副本，有一处例外：`out/pi-npm-2.pi.run.json` 里我本机全局 npm 目录的前缀写成了 `<npm-global>`（10 处：`cli.js` 的路径与 9 个调用栈帧），其余不变。`measure/measure.py` 第 31 行是本仓库的绝对路径，换一台机器要改。

## 留在本地的材料

以下在执行者本机被忽略的 `reports/pi-followup-03-20261012/` 里，不入库：

- `out/attempts/`：最终系列每次尝试的原始结果行、运行记录、进度与监听端日志。
- `out/early/`：最初几次试跑与两次被看门狗停止的长跑。
- `out/series1/`：较早的一轮完整结果及其当时的程序版本（Node `connect` 300/300、`spawn` 280/280；Bun `connect` 300/300、`spawn` 357/360）。
- `stall/out/bun-root/`、`stall/out/node-root/`：上面“失败记录”里的补测的两个实验根的原始结果（每次搜寻的合计、每个进程的运行记录与结果行、见证线程的原始行、registry、清理日志）。补测的程序、带函数名的调用栈与对照已在本目录的 `stall/`。

## 重跑

```sh
R=$(mktemp -d /private/tmp/deck-pi-r1-XXXXXX); mkdir -m 700 $R/home $R/tmp $R/bin $R/out $R/src
python3 scripts/edr_runtime.py experiment init --root $R
cp <node 24.19.0 的可执行文件> $R/bin/node; cp <官方 standalone Pi 1.1.0 的 pi> $R/bin/pi-standalone
cp docs/evidence/pi-followup-03-20261012/measure/* $R/src/
cc -O1 -o $R/bin/listen $R/src/listen.c; cc -O1 -o $R/bin/kid $R/src/kid.c
cc -O1 -shared -undefined dynamic_lookup -I<node 的 include/node> -o $R/bin/only.node $R/src/only.c
python3 $R/src/measure.py $R series node; python3 $R/src/measure.py $R series bun
python3 scripts/edr_runtime.py experiment cleanup --root $R
```
