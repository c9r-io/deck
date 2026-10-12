# Pi v0.8.0 Follow-up 02 实施证据（执行者提交，待独立验收）

对应 Follow-up 02（`docs/design/pi-v0.8.0/follow-up-02.md`）与负责人对 Follow-up 01 的独立验收（`docs/evidence/pi-followup-01-acceptance-20261012/README.md`）；两者均为负责人本地文档，不在版本控制内。本文件由执行 Agent 撰写，是提交给负责人复核的材料，不是验收结论：下面所有“自评”都需要独立验收确认，执行者不自签最终通过。

## 1. 被测构建与证据的对应关系

| 项 | 值 |
| --- | --- |
| 上一轮被复验的代码 / 证据 | `eb16c71` / `546eb45`；负责人文档原样提交为 `2153972` |
| **被测代码提交** | `8bb2b7f`（本地 `main`，未 push） |
| 本文件所在提交 | 紧接在被测代码提交之后，只增加 `docs/evidence/pi-followup-02-20261012/` 这一个目录，不改动任何被 `scripts/pi_build_identity.py` 计入的源文件；在本文件所在提交上运行 `python3 scripts/pi_build_identity.py`，`source_sha256` 仍是下面这个值 |
| 工作区 | 每次运行开始时 `dirty=false`（门禁、native、三次真实窗口各自记录在其结果里） |
| `source_sha256` | `9d6bd3ae2a846a987b588b9b7fd5fcafe246a69214699ad364fe430f23f88754`（324 个文件；算法见 `scripts/pi_build_identity.py` 头部） |
| 测试可执行文件 | `target/debug/deps/deck_app-5342d33110f51628`，SHA-256 `b6674bdf92e4e20a9dc51977d40a3b987971e0a3643e42b0b388ea28972cf7dd`（native 运行所用，亦记录在 `native-result.json` 的 `facts.build`） |
| debug 主程序（WK 载体的可执行文件） | 构建产物 `target/debug/deck-app` 的 SHA-256 `037311f83399e2b9f1204daa116583f428d367fea5d4074096ed3602114cac18`；载体在 ad-hoc 签名**之前**逐字节等于构建产物（每次运行的一项检查），签名后的哈希记录在各 `wk-*.json` 的 `facts.carrier` |
| observer 四个文件 | `observer.ts` `cf2fb8e8…e3be6f`、`signal.ts` `8f86651d…b597f1`、`lifecycle.ts` `13e6940d…03c45a`、`both.ts` `906761d9…255a61`（只有 `observer.ts` 在本轮改变；完整值在 `native-result.json` 与 `wk-*.json`） |
| Pi 运行时 | npm 安装的 Pi 1.1.0（Node.js v24.19.0）；官方 standalone 1.1.0（内置 Bun 1.3.14）。版本与文件哈希见 `native-result.json` 的 `facts.artifacts` |
| 运行位置 | Unit/Integration/Native/门禁/原语实验：开发机，macOS 27.0.1（26A434，arm64）；真实窗口（WKWebView）：隔离的测试机，同为 macOS 27.0.1（26A434，arm64） |

本轮提交（均为本地提交，未 push、未 merge、未 tag）：

| 提交 | 内容 |
| --- | --- |
| `2153972` | 负责人的 Follow-up 02 与 Follow-up 01 验收文档，原样提交（未改一字） |
| `125590e` | R03：native 驱动的输出 framing 修复与测试 |
| `c5aa34e` | R01、R02：wire v2、发送者证明（peer pid、nonce、fork 计数）、建联握手、observer、单元/真实进程/JS 测试 |
| `9705585` | native 驱动改为 v2；新增真实 Pi 工具子进程与子进程冒名场景 |
| `6712813` | 准入 worker 在无 lease 时不设定时器；修复真实进程测试里一处由调度决定的先后（见 §12） |
| `ad2b0fd` | 只改测试：R02 真实进程测试加入 Session 维度；R01 测试结尾等待对端自己的日志行（见 §12） |
| `f37cdd2` | **R01 补充修复**：执行者在装配证据前自查 `c5aa34e` 的证明时找到一个可重复的反例（§4.2），先钉成失败测试，再把“连接须晚于上次查询才算数”加入发送者证明 |
| `8bb2b7f` | 只改文字与一条测试的时间界限：`tests/edr_quiet.rs` 头部契约与三处注释里 v1 的旧措辞（“一条 stream”）改为现状，并在该契约里写明 worker 对 status socket 的那条连接。没有改动任何产品行为或检查；因为它改变了源文件哈希，它是被测提交 |

全部证据在 `8bb2b7f` 上重新生成；更早提交上的运行（包括在 `ad2b0fd` 与 `f37cdd2` 上各自已经全绿的一整套门禁，以及 `ad2b0fd` 上全部通过的 native 矩阵）一律不作为证据，列在 §12。

## 2. 判定一览（执行者自评，待独立验收）

| 项 | 自评 | 一句话依据 |
| --- | --- | --- |
| R01 继承 FD 的子进程冒用父 lease | PASS（前提与边界见 §3.6、§14） | 不再有长连接；每条消息独占一条连接。一行被当作绑定进程自己写的，须同时满足四件事：内核 peer pid 是该进程；自 Deck 上次查询该进程以来内核未报告其 fork；这条连接是在那次查询**之后**才建立的；携带 Deck 上一个应答给出的一次性 nonce。负责人的反例与执行者自己找到的反例在同一构造下都没有任何写入 |
| R02 watch 建立前的旧 hello / 缓冲帧 | PASS | `hello` 不带任何声明，只换取 nonce；watch 与查询先于 nonce；声明只能出现在一条**在那次查询之后才建立**的连接上、带该 nonce 的 `attest` 里。旧映像缓冲的 hello、帧、乃至被转交的 nonce 都不能归给新映像 |
| R03 native harness 漏读首条事件 | PASS | 只剥离 libtest 对这一个测试的确切前缀；其余位置出现事件标记、或事件不是 JSON 对象，立即报错并保留原始输出；未加任何超时 |

| 总判定 | 自评 | 说明 |
| --- | --- | --- |
| Design | PASS（含需裁决点，见 §13） | 仍是唯一生产路径（Pi extension → 现有 status socket）；传输从“每个 observer 一条长连接”改为“每条消息一条连接”，并且 Deck 的准入 worker 在需要时向**自己的** status socket 建立一条连接以取得次序标记。这两点都是对已批准设计的改动，理由与取舍在 §3、§13，由负责人裁决 |
| Implementation | PASS | R01–R03 均已实现并有对应失败测试、修复与证据 |
| P1a Signal | PASS | 发送者证明覆盖 Signal 帧；真实 Pi 两种运行时的工具子进程只带来一次重试 |
| 完整 P1 | BLOCKED | 与上一轮相同：Pi 1.1.0 无可靠的 needs-input/clear 原语；本轮未订阅 `ui_prompt`，未伪造 needs-input |
| P2 Lifecycle | PASS | lifecycle 帧与 `reset` 走同一发送者证明；`unsupported-native-id` 的粘性与不泄漏 ID 保持 |
| P3 Re-entry | BLOCKED | `PI.reentry` 仍为 `None`；未接 Claude receipt mailbox；拒绝路径的测试通过**不是** P3 PASS |
| Security | PASS（残余见 §14） | R01、R02 的反例关闭；未授予任何自动输入、重入或 scheduler/remote 权限；已识别 Pi 的自动输入 HOLD 未动 |
| Regression | PASS | 完整门禁在被测提交上逐步 exit 0（§10）；F02–F06 的回归见 §8；`test-compat-candidate` 需 macOS 14，UNRUN |
| Native E2E | PASS（范围内）；其余 UNRUN/BLOCKED 见 §15 | 真实 Pi 两种运行时完整矩阵 282 项通过；真实窗口三次运行通过 |
| Product Constitution | PASS | 无自动卡片移动、无新权限、无 Agent 语义重建；无法证明来源的行不改变任何状态；无 Pi 时不新增常驻定时器 |

## 3. 原语：能证明什么，不能证明什么

本节先于实现完成（§3.4 是在自查中补做的）。源码引用为 Apple 公开的 XNU（`xnu-12377.1.9`，`main` `f6217f89`；引用文件的 SHA-256 见本节末），每一条都在本机（macOS 27.0.1）用定向实验复核；实验程序与原始输出在 `primitive/`。

### 3.1 `LOCAL_PEERPID` / `LOCAL_PEERTOKEN` 是“最后使用者”，不是作者

- 源码：`uipc_usrreq.c:868`、`:912` 返回对端 socket 的 `last_pid`；`uipc_socket.c:400` 的 `so_update_last_owner_locked` 在 bind、listen、accept、connect、send、receive、poll、kqueue 注册处被调用（`:756`、`:836`、`:1503`、`:1643`、`:2093`、`:2630`、`:3205`、`:4002` 等），close、shutdown、getsockopt、fstat、dup 不调用。`sosend` 在 `:2093` 更新 last owner，随后在拷贝用户数据期间释放 socket 锁，字节在更后面才入队——“peer 是 P”先于“P 的字节”可见。
- 实验 B（`primitive/owner.c`、`owner.py` → `owner.json`）逐步读取服务端看到的 peer pid：
  - 子进程在继承的连接上写 → peer 变为子进程；
  - 父进程此后 `getsockopt`、`fstat`、`dup`+`close` → 不变；
  - 父进程此后**无数据可读的 `recv`（EAGAIN）、`poll`、kqueue 注册** → peer 变回父进程，而父进程一个字节也没写；
  - 父进程对子进程已连接的 socket 再 `connect`（失败，`EISCONN`）→ peer 也变为父进程；
  - 父进程在子进程之后写 → peer 为父进程，而缓冲区里两者的字节都在。
- 结论：对一条可能被多个进程持有的连接，一次（或多次）查询都不能说明缓冲区里某段字节的作者。“父进程事件循环看了一眼自己的 socket”就足以把子进程写的行“洗”成父进程的。因此任何以长连接 + peer pid 为基础的方案都被排除。

### 3.2 fork 报告先于子进程运行

- 源码：`kern_fork.c:603` `proc_knote(parent, NOTE_FORK | pid)` 位于 `:624` `task_clear_return_wait`（放行子线程）之前；`posix_spawn` 路径在 `kern_exec.c:4897` 发出 `NOTE_FORK`，同样在放行新线程（`:5078`）之前。
- 实验 A（`primitive/fence.c`、`fence.py` → `fence.jsonl`）：对一个进程注册 `EVFILT_PROC`（`NOTE_FORK`），让它创建 20000 个 `fork` 子进程和 20000 个 `posix_spawn` 子进程，每个子进程的第一件事是在继承的连接上写一个字节；服务端每读到一个子进程字节就不等待地查询队列。`unreported: 0`（两种方式各 20000 次）：子进程的第一个字节被读到时，fork 报告一定已经在队列里。
- 结论：“两次查询之间 fork 计数未变”可以推出“这段时间内没有新的子进程拿到该进程的内存副本或描述符副本”。它只对“fork 产生的副本”成立；进程主动把描述符或内容交给别人不在其内。

### 3.3 对同一 PID 重复登记不会丢事件

- 源码：`kern_event.c:1285` `filt_proctouch` 重新登记时保留尚未取走的 `kn_fflags`；`:1139` `EV_CLEAR` 对该 filter 自动设置。
- 实验 C（`primitive/rewatch.py` → `rewatch.json`）：fork 之后、exec 之后再次 `EV_ADD` 同一 PID，随后读取——事件仍在；无事件时重复登记不产生事件；对已消失的 PID 登记得到 `ESRCH`。
- 新增 Rust 测试 `procinfo::tests::a_watch_taken_while_a_process_executes_never_costs_an_earlier_one_its_report`：一个线程不停登记 watch，同时目标进程真实 `exec` 60 次；先于 exec 存在的 watch **每一次**都读到 `Executed`，exec 之后新登记的 watch 读到 `Same`。这回答了“同 PID 多 watch 登记时是否会遗漏并发 exec”：不会（落在登记瞬间的 exec 会被新 watch 一并看到，方向是 fail-closed）。

### 3.4 `accept` 按连接建立的先后交出连接

- 源码：UNIX stream 的 `connect` 在 `uipc_usrreq.c:1405` 用 `sonewconn` 建出服务端一侧的 socket，并在 `unp_connect2`（`:1578`）里对它调用 `soisconnected`；`soisconnected`（`uipc_socket2.c:226`）把它 `TAILQ_INSERT_TAIL` 到监听 socket 的 `so_comp` 队尾——这发生在客户端的 `connect()` 系统调用返回之前。`accept`（`uipc_syscalls.c:566`）取 `TAILQ_FIRST(&head->so_comp)`。所以：谁的 `connect()` 先完成，谁先被 `accept` 取走。
- 实验 D（`primitive/queue.c`、`queue.py` → `queue.json`）：
  - 第一部分（不依赖调度）：监听方先不 accept；一个进程依次建立 20 条连接，然后监听方自己建立 1 条（“标记”），另一个进程再建立 20 条；最后全部 accept——次序恰为前 20 条、标记、后 20 条（`as_connected: true`）。
  - 第二部分（并发）：8 个进程各自尽快建立 5000 条连接（每条在行里写下自己 `connect()` 调用前后的时钟），监听方单线程逐条 accept，同时由另一线程反复建立标记连接（记下每次 `connect()` 前后的时钟），共 2878 个标记。按 accept 次序检查：`behind_but_earlier: 0`（排在某个标记之后被 accept、而其 `connect()` 在该标记的 `connect()` 开始之前就已返回的连接：没有）；`ahead_but_later: 0`；与某个标记的 `connect()` 在时间上重叠的连接有 1199 条（说明确实存在并发）。
- 结论：监听方按 accept 的次序给连接编号后，“编号排在标记之后”可以推出“这条连接的 `connect()` 完成于标记的 `connect()` 开始之后”。这是一个只依赖内核队列次序的事实，不依赖任何时钟、超时或线程调度。

### 3.5 被排除的方案

| 方案 | 排除理由 |
| --- | --- |
| 长连接 + 握手时/每帧查询 `LOCAL_PEERPID`/`LOCAL_PEERTOKEN` | §3.1：值随最后使用者变化，父进程不写也能改变它；不能说明每一段字节的来源 |
| 长连接 + 逐帧“已读确认”锁步协议 | 依赖运行时在两帧之间不被调度到其它持有者，无内核事实可依；拆行/合并读取下无法定界 |
| 用 libproc 扫描对端进程族的 fd 表证明“独占” | 在途的 `SCM_RIGHTS` 不在任何 fd 表里；需要枚举其它进程，增加进程发现面（EDR）；有竞态 |
| `SCM_CREDS` 类凭据 | macOS 的 UNIX domain socket 只支持 `SCM_RIGHTS`（`uipc_usrreq.c:2502`） |
| Mach audit token / XPC | 是另一条 IPC 路径，超出“唯一生产路径” |
| 每连接一条消息 + peer pid + fork 计数 + “连接的 accept **时间**晚于上次查询”（执行者的第一版，未提交） | 自查时发现反例：observer 放弃的一条旧消息在排队后才被处理，会把 fork 计数“吃掉”；若此时父进程已创建下一条连接而 Deck 尚未 accept，accept 时间晚于那次查询，判定失效。这正是 Follow-up 点名的“排队后延迟 commit” |
| 每连接一条消息 + peer pid + fork 计数 + 一次性 nonce 链，并以“observer 任一时刻至多一条连接”为前提（执行者的第二版，`c5aa34e`…`ad2b0fd`，**已提交后被自己否决**） | 前提不成立：observer 等不到应答时会放开那条连接、另建一条；此时两条连接可以各被一个子进程持有。第一个子进程从旧连接上读到 Deck 迟到应答里的新 nonce（该 nonce 生成于两个子进程都已被计数之后），转告第二个子进程；第二个子进程在父进程尚未说话的新连接上写入伪造帧，父进程事件循环看一眼该连接，内核即报告 peer 为父进程——三项全部满足，伪造的 `turn-done` 被写入。已做成真实进程、真实内核、生产 listener 上可重复的失败测试（§4.2、`pinned-v2/`） |
| 用监听方 accept 的**时间**或“backlog 当前为空”的轮询来判断连接先后 | 取出连接与给它编号之间没有原子性，无法排除“已被取出、尚未编号”的连接；只有队列次序本身（§3.4）是确定的 |

### 3.6 采用的证明

每个绑定进程 P 有一次“查询”（look）：worker 读取 P 的 fork 计数，随后**自己向 status socket 建立一条连接**并取得它在 listener accept 次序里的编号（称为这次查询的 mark），再生成 nonce。status listener 给每条被 accept 的连接按次序编号（称为连接的 place；`agent_status.rs` 的 listener 段、`Intake::take`），worker 的这条连接走同一个 listener、同一个 dispatcher，由 dispatcher 线程直接回答编号，不经过 worker（`Service::hand`；只回答给 Deck 自己的 pid）。

一行被当作 P 自己写的，必须**同时**满足四件事（`agent_pi/transport.rs` 头部 “Whose bytes”）：

- (a) 承载它的连接，内核报告的 peer pid 是 P；
- (b) 自那次查询以来，内核没有报告 P 的 fork；
- (c) 这条连接的 place 排在那次查询的 mark 之后——按 §3.4，它建立于那次查询之后；
- (d) 它带着 Deck 给 P 的上一个应答里的 nonce（每个应答一个，只能用一次）。

为什么足够：别的进程 X 在这条连接上写了字节，X 必须持有这条连接。一条由 P 建立的连接，只会被 P 以及 P 在建立它**之后** fork 出的子进程（及这些子进程再转交的对象）持有。按 (c)，连接建立于查询之后，所以这样的子进程也是查询之后 fork 的——按 §3.2 必被计入，(b) 不成立。X 自己新建的连接，内核报告的是 X——(a) 不成立。(d) 是在此之上的一道：它把一行绑定到它之前的那一个应答，使一行只能用一次，并且第二个写者还必须读到过那个应答。

这四点不涉及“某行何时被读到、排了多久、何时 commit”，也**不依赖 observer 同时持有几条连接、何时放开哪一条**：§3.5 倒数第二行的反例里，第二个子进程持有的那条连接建立于“生成该 nonce 的那次查询”之前，(c) 不成立。

查询只在 fork 计数变化时前移：此时计数、新的 mark、新的 nonce 一起更新；取不到 mark（例如 listener 一时未能 accept）就不给出任何 nonce，消息被答复 `busy`，状态不变。(b)、(c) 或 (d) 不成立而 peer 是 P 时，回答 `again` 并给出新 nonce：不写入、不结束 lease、不责怪（Pi 每跑一个工具就会 fork，一次重试即可）；(a) 不成立时回答 `sender-mismatch`，lease 连 nonce 都不动。

证明所依赖的、关于 observer 所在运行时的前提只剩一条，如实列出（§14 第 1 点）：**建立一条连接是一步**——两种运行时都在一次调用里先后执行 `socket()` 与 `connect()`。若某个运行时恰在一次查询的瞬间、由另一线程在这两个系统调用之间 fork，则该子进程持有一条被 (c) 判为“较晚”的连接；它仍须另行得到 nonce。

observer 一侧“任一时刻至多一条连接”仍然保留并被测试（`observer.ts` 的 `line`；JS 测试统计整份测试期间同时存在的连接数最大为 1，共 200+ 条连接；`runtime-close/` 在 Node 24.19.0 与 Bun 1.3.14 上测得 `destroy()` 返回时 fd 即关闭），但它不再是证明的前提，只是让 `again` 保持为例外。

不覆盖的情形（见 §14）：进程主动把连接、nonce 或字节交给别人，或其内存可被他人读取；启动该进程的父辈（它本可以自己充当 pane 的前台程序）。

引用的七个 XNU 文件（`kern_event.c`、`kern_exec.c`、`kern_fork.c`、`uipc_socket.c`、`uipc_socket2.c`、`uipc_syscalls.c`、`uipc_usrreq.c`）的来源与完整 SHA-256 在 `primitive/xnu-sources.txt`。公开源码的版本早于本机内核，所以每一条都以本机实验为准。

## 4. R01 继承 FD 的后台子进程可冒用父 lease

### 4.1 根因与修改

**根因**：observer 为每个 Pi 保持一条长连接，Deck 把这条连接上的每一行都当作绑定进程写的；而子进程一出生就持有父进程全部描述符的副本，内核又不区分同一连接上的写者（§3.1）。

**修改**（`c5aa34e` 与 `f37cdd2`；`app/src-tauri/src/agent_pi.rs`、`agent_pi/transport.rs`、`agent_status.rs`、`procinfo.rs`、`resources/pi/observer.ts`）：

- wire v2：每条连接一条消息、一行应答；`hello`（无声明）→ nonce → `attest`（带 nonce 的声明）→ `accepted{lease, nonce, capabilities}`；此后每帧带 `lease`、上一个应答的 `nonce`、`seq`；应答 `ack`/`revoked`/`again` 都带下一个 nonce。
- `procinfo::InstanceWatch` 在同一个 kqueue 登记上增加 `NOTE_FORK`，`forks()` 给出计数。
- status listener 给每条被 accept 的连接编号（place），并把编号随行交给 Pi transport；Deck 自己的一行（`agent_pi::place_line`，`source` 为 `deck`，任何 observer 的行都不可能与它相同）由 dispatcher 直接回答该连接的编号，只回答给 Deck 自己的 pid。
- `transport::commit`：按“lease 存在 → peer pid → lease 存活 → 运行实例 → fork 计数、连接的 place、nonce（同时轮换 nonce；fork 计数变化时前移查询并取新的 mark）→ 行是五种帧之一且应答未超时 → pane 列表 → 内核现状 → 写入”的次序；发送者被证明之前，除轮换 nonce、前移查询与内核自己的 exec/exit 报告外，不结束、不改变 lease。
- listener：三个协议族都是一连接一消息，首行之后还有字节一律 `trailing-bytes`。
- observer：一次只存在一条连接；`again` 用新 nonce 重发同一条消息（有限次）；无应答、意外应答、队列溢出时放弃 lease，并在 Deck 可能仍持有它时用一条 `reset` 告知。`f37cdd2` 没有改动 observer 的行为，只改了它文件头的一句话。

### 4.2 执行者自查发现的反例（`c5aa34e`…`ad2b0fd` 上可重复）

在 `ad2b0fd` 上全部门禁与 native 矩阵都已通过之后，执行者在装配证据前逐条复核 §3.6 的证明，发现“observer 任一时刻至多一条连接”并不能推出“Deck 生成 nonce 时，P 没有别的、尚未说话的连接”——observer 等不到应答（2 秒）就会放开旧连接、另建一条。构造：

1. P 已绑定并 `working`。P 建立连接 j，fork 出子进程 X（X 持有 j）；P 在 j 上写下自己的心跳，随后放开 j（X 仍持有）。
2. P 建立连接 k，fork 出子进程 Y（Y 持有 k）；k 上还没有任何字节。
3. Deck 读到 j 上的心跳：peer 是 P、nonce 正确，但 fork 计数变了——回答 `again`，并把新 nonce N1 写到 j 上。N1 生成于 X、Y 都已被计数之后。
4. X 从 j 上读到 N1，转告 Y。Y 在 k 上写入带 N1 的 `turn-done`。P 的事件循环看一眼 k。
5. Deck 读 k：peer 是 P、nonce 是 N1、自 N1 生成以来没有 fork——`c5aa34e` 的三项全部满足，**伪造的结束被写入**（store 写入 +1、episode +1、通知 +1、run 被关闭）。

这正是 Follow-up 点名的“排队后延迟 commit”的一种形态：它不需要父进程配合，只需要两个子进程和一次迟到的应答。执行者按“先钉失败测试、再修复”的次序处理：

- 两条真实进程测试先在 `ad2b0fd` 的 transport 上运行并失败，输出与当时的测试 diff 保存在 `pinned-v2/`（`failing-left.log`、`failing-before.log`、`tests-on-ad2b0fd.diff`）。两条失败都是同一句断言：`what a child wrote was taken for its parent's`，左右两侧的差别即上面第 5 步列出的写入。
- 修复即 §3.6 的 (c)。修复后同一构造的结果见下表最后两行。

### 4.3 期望与实测

负责人的反例：前台父进程绑定并 `working`，fork；子进程 `setsid()` 后在继承的连接上发 `turn-done` 与 lifecycle `snapshot`。

| 构造 | 上一轮（`eb16c71`） | `c5aa34e`…`ad2b0fd` | 本轮（`f37cdd2` 起） |
| --- | --- | --- | --- |
| 子进程在继承的连接上写，其后无人碰该连接 | ACK，卡片变为 `turn-done` | `sender-mismatch`；无写入 | 同左 |
| 同上，但父进程随后“看了一眼”该连接（poll） | 同上 | `again`（原因恰为 fork）；无写入 | 同左 |
| 子进程用自己新建的连接，带上它知道的一切 | —— | `sender-mismatch` | 同左 |
| 子进程从父进程在途连接上偷读到一个新 nonce，再用自己的连接 | —— | `sender-mismatch` | 同左 |
| §4.2 的构造：一个子进程从父进程放开的连接上读应答，转告持有父进程下一条连接的另一个子进程 | —— | **伪造的 `turn-done` 被写入** | 这个构造要求 k 在第 4 步之前不被 accept（否则 Deck 会在 Y 写入的瞬间读到那一行，peer 是 Y）。而只要 k 还没有被 accept，worker 排在它后面的那条连接也不会被 accept：取不到 mark，心跳被答复 `busy`，不给出任何 nonce；X 读到的只有拒绝。Y 用它出生时带着的 nonce 写入，遇到的是造出两个子进程的那两次 fork：`again`（原因 `fork`）；无任何写入 |
| 更强的对手：子进程持有一条在查询**之前**建立的连接，并且被**告知**了 lease 当前的 nonce（任何子进程都无法自行得到）；父进程看一眼该连接并亲自写下行尾 | —— | 伪造的 `turn-done` 被写入 | peer、nonce、fork 计数全部正确——`again`，原因恰为 `place`；无任何写入 |

### 4.4 测试

每一项拒绝都在只增不减的计数器上断言“期间没有任何写入”，而不只是检查最终状态：`agent_status` 的 Pi 写入次数、lifecycle 写入次数、下一个 episode 编号、通知发布次数、lease 结束列表、每个 lease 的 run/seq/能力（测试里的 `written()`）。

| 层 | 测试 | 覆盖 |
| --- | --- | --- |
| Unit（内核回答由桩精确给定） | `a_line_is_the_observer_s_own_only_with_its_peer_its_nonce_and_no_fork_together` | peer、nonce、fork 各缺其一 × Signal/lifecycle/reset/ping；`again` 的原因被逐条断言；父进程自己的下一帧写入且恰为下一个 episode；同一行重放不写第二次 |
| Unit | `a_line_counts_only_on_a_connection_made_after_deck_last_looked` | peer、nonce、fork 全部正确，但连接建立于查询之前 × 四种帧：`again`，原因恰为 `place`，零写入；同样的行在查询之后建立的连接上被写入且恰为下一个 episode；`attest` 同理（不识别、不绑定） |
| Unit | `without_a_mark_no_nonce_is_made_and_nothing_changes` | 取不到 mark：`hello`、fork 之后的帧、fork 之后的 `attest` 均 `busy`；nonce 未轮换、fork 计数基线未动、lease 未结束；无 fork 的帧不需要新的 mark |
| Unit | `an_attest_binds_only_with_the_nonce_its_own_process_was_given`、`a_nonce_waits_a_bounded_time_and_a_bounded_number_wait` | 无 hello、别人的 nonce、用过的 nonce、过期、hello 与 attest 之间 fork；32 个未决 nonce 的上限 |
| Unit（服务层） | `deck_s_own_line_is_answered_with_its_place_and_reaches_no_worker` | worker 卡在一次不返回的 pane 列表里时，Deck 自己的一行仍被立即回答编号，不进入 worker 队列 |
| Unit（wire） | `deck_s_own_line_is_no_observer_s_and_its_answer_is_a_place` | 该行逐字节唯一；family 解析器把它拒为 `bad-frame`；六种近似行都不被当作它 |
| Listener（生产 accept/首行/dispatcher，真实 socket） | `agent_status::tests::a_connection_s_place_is_the_order_in_which_it_was_made` | 先后建立四条连接，行按相反次序到达、其中一条始终沉默：编号仍是建立的次序；`mark_at` 得到的编号排在它们之后 |
| **C adversarial fixture + 真实内核 + 生产 listener/transport**（`scripts/pi/exec-peer.c`，真实 tmux pane） | `r01_no_child_of_the_observer_writes_anything_for_it` | 7 类先后次序、共 20 轮（见下表）；pane B 的 observer 全程不受影响 |
| 同上 | `r01_what_one_child_read_on_a_connection_its_parent_left_writes_nothing_on_another` | §4.2 的构造，逐步由测试决定先后：`busy`（1 次）、`again` 原因 `fork`；零写入；父进程自己的结束随后写入且恰为下一个 episode |
| 同上 | `r01_a_connection_made_before_deck_last_looked_is_nobody_s_whatever_it_carries` | §4.3 最后一行：`again` 原因恰为 `place`；零写入；pane 里的程序发送 Deck 自己的那一行被拒 `sender-mismatch`，得不到编号 |
| JS（真实 observer 文件，对端为逐连接的 Deck 桩） | `pi-observer.test.mjs` 28 项 | nonce 链、`again` 重试与放弃、无应答后的告别与重新绑定、一次一条连接 |
| **真实 Pi**（Node 与 Bun） | native `tool_children` | Pi 自己的 shell 工具（真实子进程）只带来一次 `again`；工具里运行的程序用 pane 的环境直连 status socket 冒名，被拒 `not-foreground` 与 `sender-mismatch` |
| 真实 Pi + C fixture | native `exec_peer` D 段 | 已绑定程序的子进程在继承的连接与自己的连接上各写一次，均 `sender-mismatch`；父进程自己的结束帧恰为下一个 episode |

`r01_no_child_of_the_observer_writes_anything_for_it` 的 20 轮。测试用一个开关让生产 listener 暂不 accept（或只 accept 等得最久的那一条），从而由测试而不是调度决定“Deck 读到第一个字节时已经发生了什么”；生产代码没有为此做任何改动：

| 次序 | 子进程种类 × 伪造帧 | 结果 |
| --- | --- | --- |
| 1 子进程在继承的连接上写，父进程随后 poll 该连接 | 同进程组 / `setsid` / fork 后 exec 另一程序 × `turn-done` / lifecycle `snapshot` / `reset`（9 轮） | 均 `again`，原因 `fork`；零写入；父进程自己的帧随后写入（先因 nonce 被轮换再被要求重发一次） |
| 2 同上，父进程不再碰该连接 | `setsid` × `turn-done` | `sender-mismatch`；零写入 |
| 3 子进程用自己的连接 | `setsid` / exec × 三种帧（6 轮） | `sender-mismatch`；零写入 |
| 4 子进程偷读父进程在途消息的应答，再用自己的连接 | `setsid` × `turn-done` | 父进程的消息 `again`（fork）；子进程 `sender-mismatch`；零写入 |
| 5 子进程的行分两段到达，父进程在每段之后 poll | 同进程组 × `turn-done` | `again`（fork）；零写入 |
| 6 子进程的行后面紧跟父进程自己的行（合并） | 同进程组 × `turn-done` | listener 拒绝 `trailing-bytes`；零写入 |
| 7 子进程在继承的连接上写；父进程放开它，在新连接上发正常心跳 | `setsid` × `turn-done` | 子进程 `sender-mismatch`；父进程心跳 `again` 后 ACK；除心跳占用一个序号外无变化 |

## 5. R02 watch 建立前的旧 hello / 缓冲帧可归给新映像

**根因**：watch 在 hello 交给 transport 时才登记；此前已写入 socket 的 hello 及其后的帧，被当作登记时刻正在运行的映像所说。

**修改**（`c5aa34e`；`f37cdd2` 把 §3.6 的 (c) 同样用于 `attest`）：`hello` 不携带任何声明，worker 处理它时先登记 watch（exec/exit/fork）、做一次查询并取 mark，再生成 nonce 应答；声明只在 `attest` 中出现，且必须带这个 nonce、通过 §3.6 的四点（因此它所在的连接建立于 watch 开始之后，上面的字节也就写于其后）、并且 watch 自登记以来没有报告 exec。于是：被接受的声明写于 watch 建立之后，属于被绑定的运行实例。nonce 只说明“这一行写于何时之后”，从不单独充当进程身份——把它交给新映像没有用（见下表第 4 行）。已有的“watch 建立后的 exec/exit 撤销、每帧 commit 检查、所有 current-evidence 读取检查”原样保留。

**期望与实测**（`r02_what_an_earlier_image_wrote_or_was_given_is_not_the_new_image_s`：C fixture + 真实内核 + 生产 listener；先后由“listener 暂不 accept”的开关决定）：

| 时序 | 结果 |
| --- | --- |
| 旧 hello 已缓冲 → 同 PID exec → 新映像沉默 → Deck 才读 | Deck 监视的是新映像，回一个无人使用的 nonce；零写入，不识别为 Pi，无 lease |
| 旧 hello + `working` 帧缓冲在同一连接 → exec → Deck 才读 | `trailing-bytes`；零写入；不产生 nonce |
| 已绑定（两种能力），已报告 Session 与 `working`；`turn-done` 帧与另一个 Session 的 `snapshot` 帧各缓冲在一条连接上 → exec → Deck 才读（PID、lease、nonce 全对，无 fork） | 两帧均 `peer-executed`；lease 结束；episode 计数未动（未曾写入结束）；新映像没有 `pi_session`，旧 Session 与帧里的 Session 都不留存 |
| 旧映像拿到 nonce → exec，并把 nonce 交给新映像 → 新映像 `attest` | `peer-executed`；零写入 |
| hello 写了一半 → exec → 新映像写完另一半、读到 nonce、自己 `attest` | 绑定成立——绑定的是**新映像自己**：hello 不含声明，旧映像没有任何东西落在其后 |
| 接受后真实同 PID exec，新映像沉默 | 所有读取方立即撤回；worker 在下一次巡查时以 `peer-executed` 结束 lease，无需任何帧或 poll（`a_real_exec_inside_the_observer_s_pid_ends_its_lease` 的 B 段；native `exec_peer` B 段） |
| 登记 watch 附近的 exec | 先于登记：watch 即为新映像的，nonce 在其后生成；晚于登记：被报告。落在登记瞬间：由 §3.3 的并发测试说明不会漏报 |
| `attest` 所在的连接建立于 hello 的那次查询之前（Unit：`a_line_counts_only_on_a_connection_made_after_deck_last_looked` 的后半） | `again`，原因 `place`；不识别、不绑定；在其后建立的连接上同一 `attest` 绑定成立 |

Unit 层对应 `what_an_earlier_image_said_or_was_given_is_not_the_new_image_s`、`a_peer_that_executed_another_image_is_not_the_observer_any_more`。普通 fork、子进程 exec、磁盘文件被替换都不被当作父实例 exec（`procinfo` 既有测试与 native `exec_peer` C 段）；内核不可问时仍 fail-closed（`no-instance`）。正常 Pi 的启动、重连、reload 见 native 矩阵（§9）。

## 6. R03 native harness 漏读 libtest 前缀后的事件

**根因**（确认）：`Tap.read` 只接受以 `@tap ` 开头的行。libtest 以 `--nocapture` 运行单个测试时，会在测试开始前写出 `test agent_pi::tests::native::tap ... ` 且不换行，harness 写的第一行就接在它后面；第一行是 `ready` 还是一次 store 采样由线程调度决定，于是首条事件被丢弃，丢的是哪一条不确定。

**修复**（`125590e`，`scripts/pi_native.py`）：`tap_event` 只剥离这一个测试的确切 libtest 前缀；事件标记出现在行内任何其它位置、或事件不是 JSON 对象，抛出 `Framing`，读线程记录原因，此后任何等待立即以该原因失败（不是等到 30 秒超时），并且其后的行不再被当作事件；backend 的每一行原样保存在 `tap-N.stdout`。没有增加超时，没有忽略解析错误，没有改动任何产品断言，也没有从任意文本里猜 JSON。

**测试**（`scripts/test_release_tools.py::PiNativeTapTests`，4 项，用一个原样输出给定文本的假 backend 驱动真实的 `Tap` 类）：`ready` 为首条、采样为首条（不丢失，且其后的 `ready` 被找到）、普通行与非事件行、六种坏行（行中事件、前导空格、别的测试的前缀、重复前缀、非 JSON、非对象）、restart（第二个 backend 生命的首行同样带前缀）、framing 在启动时或稍后被破坏时立即失败并保留原始行。

native 矩阵在修复后重新运行（§9）。首次失败与设施修复分列在 §12。

## 7. 协议版本与旧 runtime

- wire v1 从未发布（v0.8.0 的 Pi 集成尚未进入任何 Nightly/Stable）。本轮把 wire 与 observer 版本都改为 2，握手与帧格式不兼容，**不并存**：v2 的 Deck 不接受任何 v1 行。
- `f37cdd2` 没有改变 observer 所说的 wire：仍是 v2，observer 发送和读取的行一字未变。新增的是 Deck 自己在同一 socket 上的一行（`source` 为 `deck`）及其应答（`place`），observer 既不发送也不会收到；pane 里的程序发送它只会得到 `sender-mismatch`。
- 仍在内存里持有 v1 observer 的 Pi（它在 Deck 被替换前加载了旧文件）：其 hello/帧被回答 `bad-version`，并且用**它自己的 framing**（`"v":1`）回答——v1 observer 只读自己版本的行，并把 `bad-version` 当作本 runtime 内的终局，于是停止；Pi 下次启动或 reload 时加载本构建的 observer。测试：`another_version_is_refused_in_its_own_framing`、`a_line_that_breaks_the_wire_…` 的 v1 段（零写入、不产生 nonce）。
- 这样的 Pi 仍按进程名被当作 Pi：卡片显示 `unavailable`，自动输入的 HOLD 照旧。Settings 不会为它显示 “needs reload”，因为 v1 行的发送者没有被证明（如实列为限制）。
- 反向（v2 observer 遇到旧 Deck）：旧 Deck 的 v1 解析器回答 `bad-version`，v2 observer 同样视为终局。

## 8. 回归：负责人已通过的 F02–F06

| 项 | 本轮状态 |
| --- | --- |
| F01 的既定裁定：kqueue `NOTE_EXEC`/`NOTE_EXIT` 作为登记后的连续性事实 | 保留；同一登记增加 `NOTE_FORK`。exec 后旧 Pi proof 清除、内核不可读不显示 `connected` 的测试原样通过 |
| F02 `unsupported-native-id` 粘性、不泄漏 ID | 逻辑未改；`an_id_this_build_does_not_keep_makes_sessions_unsupported_and_leaves_the_runs`、JS 两项、native `unsupported_ids` 通过 |
| F03 能力只收紧、互不影响、重新启用需 reload | Deck 侧仍然立即生效。**observer 得知的时刻变了**：不再有不请自来的 `capabilities` 行，而是在它下一条消息的应答里（绑定时在 `accepted` 里，空闲时最迟一个心跳 5 秒）；在途帧仍回答 `revoked`。见 §13 第 4 点 |
| F04 可见 UI | 本轮没有改动任何 UI/CSS/文案文件；真实窗口三种模式仍重新运行（§9） |
| F05 证据对应最终构建 | 沿用同一套构建身份；本文件 §1 |
| F06 首行准入有界 | 32 个在读名额、绝对期限、有界 dispatcher 队列、单一 dispatcher 线程、闭合 drop reason、按首行完成顺序 hand-off：未改。accept 循环多做一件事：给连接编号（一个原子计数，不读、不等）。worker 自己的那条连接与任何客户端一样占用一个首行名额并受同样的期限约束；名额或队列已满时它被直接关闭，worker 得不到编号，相应的 Pi 消息被答复 `busy`。`agent_status` 的 listener 测试按 v2 调整后通过；新增 drop reason `pi-no-challenge`、`pi-sender-mismatch` |
| hook/receipt 取证与存活顺序 | 未改 |
| 自动输入 HOLD、`PI.reentry=None`、不接 receipt mailbox | 未改；`signal_census`、`edr_quiet` 的 Pi 相关约束通过（`globalThis` 等禁用词表未放宽） |

## 9. 分层覆盖

| 层 | 内容 | 结果 |
| --- | --- | --- |
| Unit / Integration（Rust，桩内核） | `agent_pi::` 70 项（另 1 项为 native harness，ignored）；`agent_status` listener 测试；`procinfo` watch 测试 | 见 §10 `llvm-cov` 步骤 |
| C adversarial fixture + 真实内核 + 生产 listener/transport | `real_processes_bind_only_as_their_own_pane_s_foreground_program`、`a_real_exec_inside_the_observer_s_pid_ends_its_lease`、`r01_…` 三条（20 轮 + §4.2 的构造 + “被告知 nonce”的构造）、`r02_…`（5 段） | 通过（§10 的门禁）；另在 `f37cdd2` 的代码上把这 6 条连续运行 20 次、其中新增的 2 条再连续运行 30 次，全部通过 |
| JS（真实 observer 文件） | `app/ui/test/pi-observer.test.mjs` 28 项 | 通过（含在 `scripts/ui-tests` 内） |
| Python（驱动自身） | `PiNativeTapTests` 4 项 | 通过 |
| **真实 Pi，Native E2E**（npm/Node 24.19.0 与 standalone/Bun 1.3.14，生产 listener/transport/store/poll/Settings 命令） | `scripts/pi_native.py` 全部 20 个场景 | 282 项通过，0 失败；清理 exit 0（`native-result.json`、`native-scenarios.json`） |
| **真实窗口（WKWebView，生产 UI + 真实 Pi）** | `pi-live` npm、`pi-live` standalone、`pi-layout` | `pi-live` npm：`smoke-verdict` PASS (16/16 expected checkpoints, 16 lines)，操作员检查 19/19；`pi-live` standalone：`smoke-verdict` PASS (16/16 expected checkpoints, 16 lines)，操作员检查 19/19；`pi-layout`：`smoke-verdict` PASS (16/16 expected checkpoints, 21 lines)，操作员检查 7/7；三次清理均 exit 0（`wk-*.json`、`wk-*.verdict.txt`） |

native 矩阵与上一轮相比的变化：`eof` 这个结束词不再存在（被杀死的 Pi 是进程退出 `generation-mismatch`，沉默的是 `stale`）；新增 `tool_children`；`exec_peer` 改为 v2 并增加 D 段；`frozen`、`disable`、`capabilities`、Deck 重启的检查按“observer 在下一条消息的应答里得知”调整。`f37cdd2` 与 `8bb2b7f` 都没有改动 native 驱动。真实 Pi 的工具子进程实测：两种运行时各被要求重发 1 次（`facts.tool_children`），lease 不变。

### 复现命令

在仓库根目录、被测提交上（cargo 命令用隔离的 `HOME`，并先 `unset TMUX TMUX_PANE DECK_STATUS_SOCK`）：

| 目的 | 命令 |
| --- | --- |
| R01/R02：C fixture + 真实内核 + 生产 listener | `cargo test --locked --manifest-path app/src-tauri/Cargo.toml --bin deck-app agent_pi::transport::tests::r0 -- --test-threads=1` |
| 发送者证明的判定表 | 同上，过滤 `agent_pi::transport::tests::a_line_` |
| 取不到 mark 时 | 同上，过滤 `agent_pi::transport::tests::without_a_mark` |
| 建联握手 | 同上，过滤 `agent_pi::transport::tests::an_attest_binds_only` |
| listener 的编号 | 同上，过滤 `agent_status::tests::a_connection_s_place` |
| watch 并发登记不丢 exec | 同上，过滤 `procinfo::tests::a_watch_taken_while` |
| observer（一次一条连接、nonce 链、`again`） | `node --test app/ui/test/pi-observer.test.mjs` |
| R03 | `python3 -m unittest scripts.test_release_tools.PiNativeTapTests` |
| 真实 Pi | `cargo test --locked --manifest-path app/src-tauri/Cargo.toml --bin deck-app --no-run`，再 `python3 scripts/pi_native.py --test-binary <上一步打印的可执行文件> --standalone <官方 standalone 解包目录> [--only tool_children,exec_peer]` |
| §4.2 的反例在修复前失败 | 在 `ad2b0fd` 的一份检出里应用 `pinned-v2/tests-on-ad2b0fd.diff`（只含测试与测试设施三个文件；例如 `git show <本文件所在提交>:docs/evidence/pi-followup-02-20261012/pinned-v2/tests-on-ad2b0fd.diff \| git apply`），再运行第一行的命令：`r01_what_one_child_…` 与 `r01_a_connection_made_before_…` 两条失败，输出即 `pinned-v2/failing-*.log` |
| 原语实验 | 新建一个 `/private/tmp` 下的根目录并 `python3 scripts/edr_runtime.py experiment init --root <root>`；`cc -O1 -o <root>/bin/fence primitive/fence.c`、`cc -O1 -o <root>/bin/owner primitive/owner.c`、`cc -O1 -o <root>/bin/queue primitive/queue.c`；`python3 primitive/fence.py <root> fork 20000 scripts/edr_runtime.py`（`spawn` 同理）、`python3 primitive/owner.py <root> scripts/edr_runtime.py`、`python3 primitive/rewatch.py <root> scripts/edr_runtime.py`、`python3 primitive/queue.py <root> scripts/edr_runtime.py 8 5000`；结束后 `experiment cleanup` |

fixture 的步骤语言写在 `scripts/pi/exec-peer.c` 文件头。负责人上一轮的 `inherited-fd-probe.py` 说的是 v1 的长连接，在本构建上得到 `bad-version`；与它等价的构造是 `r01_…` 的第 2、3 类次序，更强的构造（父进程事后 poll、偷读应答、两个子进程接力、被告知 nonce）是第 1、4、5 类与新增的两条测试。

## 10. 门禁

`gates.json` 是在被测提交上按 `.github/workflows/gate.yml` 的顺序逐步执行的记录（每步的命令、exit，以及测试数与覆盖率）：30 步全部 exit 0，开始与结束时工作区都干净、commit 都是 `8bb2b7f`（2026-10-12T10:02:35Z – 2026-10-12T10:16:40Z）。

| job | 步骤 | 结果 |
| --- | --- | --- |
| static | 内置 tmux 的 SHA-256、Bergamot 补丁、`cargo fmt --check`、JS 语法、`scripts/ui-tests`、三个 Swift bridge 测试（speech、notification、smoke guard：5 个变异体全部被抓住）、`check.mjs`、Python 工具测试、`check-workflows`、`cargo audit`、tunnel-helper 的 fmt/clippy/test/audit | 全部 0；ui-tests 781/781；Python 181 项 OK；tunnel 28 通过；两次 audit 无漏洞（应用锁文件有 8 条既有的 allowed warning） |
| rust | `cargo clippy --workspace --all-targets --all-features -D warnings`、CE 探针环境、`cargo llvm-cov … --fail-under-lines 75 --fail-under-functions 75 … -- --test-threads=1`、coverage 的 sidecar provenance、CE Full Local parity 与 verdict | 全部 0；1517 通过 / 13 ignored（其中 `deck-app` 单元测试 1243 通过）；lines 85.77%、functions 83.72%；`full_local_parity:ci: pass` |
| release | `cargo build --release`、`check_local_translation_binary.py`、`check-edr-binary`、`package-compat-candidate` | 全部 0；EDR 词表干净 |
| after | 重新构建 debug 主程序与测试可执行文件、debug 的 sidecar provenance | 全部 0 |

关于最后一行：release 构建会覆盖各构建上下文共用的 sidecar 与前端产物，所以门禁结束时重新做了一次 debug 构建；native 矩阵与真实窗口用的就是这次构建出的两个文件（§1 的哈希）。

适用性说明：

- `scripts/test-compat-candidate`（在 macOS 14 上经 LaunchServices 启动候选包）：**UNRUN**。脚本自己要求 macOS 14，本轮可用的两台机器都是 macOS 27；它的前一步 `scripts/package-compat-candidate`（打包，含 Pi observer 四个文件）已运行，exit 0。
- `release_channels.py gate-results` 是 CI 各 job 的汇总步骤，本地无对应物。
- 没有创建 candidate、tag、feed 或任何发布物；没有 push。

## 11. 清理审计

- **登记与停止。** 每次运行把自己启动的 tmux 服务与进程登记在该运行根的 experiment registry 里，结束时只经 registry 停止。最终四次运行（native 在开发机；三次真实窗口在测试机，由同一个操作员脚本在那里执行并记录在 `wk-*.json`）的清理结果都是 exit 0、没有来自根的进程、没有遗留 socket。本轮没有使用过按 PID 的 kill、`pkill` 或 `tmux -L`。
- **未经 registry 的两次测量（如实说明）。** `runtime-close/`（描述符关闭的测量）与 `module-instances/`（模块实例的测量）各自启动的只是自行结束的短进程（一个本地监听脚本、Node/Bun/Pi 的一次性运行），它们的可执行文件不在实验根之下，registry 按规则不登记这类进程；运行结束后逐一确认没有残留进程。四个原语实验（A–D）的 C 程序都在各自的实验根下编译并登记。
- **Rust 的真实进程测试**（`agent_pi::transport::tests` 里用到 tmux 的 6 条，以及钉住失败时运行的 2 条）各自在临时目录下建立自己的目录与 tmux 服务（以完整 socket 路径寻址），由测试自己停止并删除，失败（panic）时也经其析构执行；全部运行结束后检查没有遗留目录、tmux 服务或进程。
- **开发机。** 本轮被替代的 8 个运行根在核实后移入开发机的废纸篓（各自唯一命名，`<根名>-fu2-<时间戳>`），没有做不可逆删除：有 registry 的根，`show` 确认没有存活的登记项；全部要求本机没有任何进程的可执行文件或命令行指向该根。清理脚本第一次运行时把它自己的 `grep` 算作“指向该根的进程”，于是一个都没有移动（方向是保守的）；修正这项检查后第二次运行完成移动。清单：`deck-pi-r2-quOUDk`（原语实验 A–C）、`deck-pi-q-o5yZfj`（原语实验 D）、`deck-a2-JRZXgz` 与 `deck-pi-mod-iRyWzD`（两次测量）、`deck-pi-ucykcq7n`、`deck-pi-b3vi4tm4`、`deck-pi-4fgt6qx6`（未提交代码上的 native 试跑）、`deck-pi-x1lfec1y`（`ad2b0fd` 上被取代的 native 全矩阵）。
- **保留。** 最终四个运行根（native `deck-pi-ebgc8mkt`；真实窗口 `deck-pi-live-vpay6rou`、`deck-pi-live-fmvimv7g`、`deck-pi-live-eqlnvvbj`；私有日志，不入库）；`/private/tmp/deck-pi-xpgqUm`（官方 standalone Pi，各轮运行都用它）；前两轮证据的根（`deck-pi-ccmdhw54`、`deck-pi-live-13tv70bv`、`deck-pi-live-a1_wa3mf`、`deck-pi-zxpp_mb6`、`deck-pi-live-shxag3xf`、`deck-pi-live-3ra6o90z`、`deck-pi-live-nri6m51z`）与 `deck-pi-iJiyNU`。按时间与内容属于负责人两次复验的根与文件（`deck-pi-accept-n_kowcmb`、`deck-pi-3sql7ynm`、`deck-pi-hm_25dtd`、`deck-pi-lufj0f2j`、`deck-pi-7p3p76lw`、`deck-pi-sjgfjj3z`、`deck-pi-tjhiidf_`、`deck-pi-y_hsxyfz`、`deck-pi-review2-*`、`deck-pi-review-ui.log`、`deck-pi-followup-review-ui.log`）没有被修改或移动。
- **测试机。** 运行前后各清点一次（不输出其主机与账户信息）：已安装的 Deck 前后都在运行（各 1 个进程），每次运行自己的检查也确认“同样的进程、数据目录里同样的文件名”；`/private/tmp` 下没有遗留的运行根；没有任何进程来自运行根；控制台未锁定。本轮 3 个运行根在测试机的废纸篓里（连同前两轮的 24 个，共 27 个 `deck-pi-live-*`）；执行者没有清空或整理任何废纸篓。
- **未触碰。** 没有修改真实的 Pi 或 Deck 配置与数据（每次运行都在自己的 HOME 下）；没有安装任何全局工具；没有改动系统安全设置、TCC 或 launchd；没有 push、merge、tag、release 或任何发布。

## 12. 原始失败与设施修复（单列，不计入产品测试结果）

| # | 现象 | 性质 | 处理 |
| --- | --- | --- | --- |
| 1 | 在被复验的代码（`eb16c71`）上钉住的两条失败测试：R01 子进程的帧被 ACK；R02 新映像被显示为 `working` | 产品缺陷（即 R01、R02）的可重复反例 | `pinned-v1/`（测试源码、给 fixture 加的步骤、失败输出）；本轮修复 |
| 2 | 执行者第一版实现（peer pid + fork 计数 + accept 时间）被自己否决 | 设计缺陷，未提交 | §3.5；改为 nonce 链 |
| 3 | native 全矩阵首次运行 281 通过、1 失败：`B08 a restarted Deck knows no session` | 测试设施：检查读取的是“此刻”的状态，而 v2 的 observer 在 Deck 回来后一两秒内就自行重新绑定并报告了 snapshot | 检查改为读取新 backend 生命的第一条采样（它开始时持有什么）；断言含义未变（`9705585`） |
| 4 | `r01_…` 在约 12 次运行中失败 1 次：期望 `again`，得到 `sender-mismatch` | 测试设施：子进程写完后接着阻塞读应答，而“读”也是一次使用；当它恰好发生在父进程 poll 之后，内核又把子进程报告为 peer。先后由调度决定 | 子进程改为在测试发出信号后才读（此时 Deck 已应答）；连续 30 次通过（`6712813`）。这个现象本身是 §3.1 的又一例证 |
| 5 | JS 测试首次运行 1 项失败 | 测试自身的期望对象写错（`undefined` 键） | 更正期望 |
| 6 | Rust 测试首次运行 2 项失败 | 测试自身的断言不成立（按进程名识别为 Pi、不兼容版本会被识别） | 更正断言为计数器比较 |
| 7 | `signal_census` 两项失败 | 普查钉住了被改名的函数 `parse_frame` | 普查表更新为新的函数名，约束内容不变 |
| 8 | clippy `doc_lazy_continuation` | 模块头里的编号列表 | 改写为段落 |
| 9 | 第一次门禁在 `9705585` 上运行到 static 作业结束时被执行者停止 | 为加入“无 lease 时不设定时器”的改动；停止发生在作业之间 | 在 `6712813` 上完整运行，30 步全部 exit 0（被后来的运行取代，不作为证据） |
| 10 | `r01_…` 在 12 次运行中失败 1 次：结尾断言 pane B 的日志里有 `"ack":2` | 测试设施：B 的 lease 一结束测试就去读 B 的日志，而 B 是在 Deck 应答**之后**才把应答写进日志 | 测试改为等待该日志行；连续 40 次通过（`ad2b0fd`）。未改产品代码与产品断言 |
| 11 | **执行者第二版实现（`c5aa34e`…`ad2b0fd`）的证明有一个漏洞**：在 `ad2b0fd` 上门禁 30 步全部 exit 0、native 矩阵 282 项全部通过之后，自查发现 §4.2 的反例，并在真实进程上复现（伪造的 `turn-done` 被写入） | **产品缺陷**（R01 修复不完整），由执行者自己发现；门禁与矩阵都没有覆盖到这一先后次序 | `pinned-v2/`；`f37cdd2` 修复；`ad2b0fd` 上的全部运行结果作废 |
| 12 | 为 `f37cdd2` 新写的 Unit 测试首次运行 3 项失败 | 测试自身的断言不成立：pane B 的前台程序名就是 `pi`，按进程名即被识别（与第 6 行同类）；lease 编号在一个测试进程内不从 1 重新开始 | 断言改为比较写入计数与存活 lease 数、从应答里读取 lease 编号 |
| 13 | 连续运行脚本第一次调用时 30 次全部 exit 1 | 设施：给 `cargo test` 传了两个位置参数（它只接受一个，其余要放在 `--` 之后）；没有任何测试被运行 | 更正调用方式后重跑（§9 的 30 次与 20 次） |
| 14 | `f37cdd2` 上门禁 30 步全部 exit 0 之后，执行者发现 `tests/edr_quiet.rs` 的头部契约与三处注释仍写着 v1 的“一条 stream”（`c5aa34e` 改 wire 时漏改），并且该契约没有提到 worker 对 status socket 的那条新连接 | 契约文字滞后于行为（不是行为缺陷；该文件的检查本身一直通过） | `8bb2b7f`：只改文字，并把一条新测试所用的应答期限改为生产值，使它不依赖被搁置的 `attest` 等了多久。因为源文件哈希随之改变，门禁、native 矩阵与真实窗口全部在 `8bb2b7f` 上重跑；`f37cdd2` 上那次门禁不作为证据 |

## 13. 需要负责人裁决的设计点与偏差（执行者不自批）

1. **长连接改为每条消息一条连接。** 设计 README 原写“一个 observer 一条 stream”。§3.1 说明在可被 fork 继承的长连接上没有任何公开原语能给出逐段字节的作者；改为短连接后，三个协议族在这个 socket 上形态一致。代价：每条消息一次 connect；lease 的存活不再有“连接断开”这个即时信号，改由进程退出/exec 的内核报告（即时可读、worker 每秒巡查一次）、15 秒无帧（`stale`）、observer 的 `reset` 三者给出。
2. **Deck 向自己的 status socket 建立连接。** 每个 `hello`、以及每条遇到 fork 计数变化的消息，worker 都会向本进程的 status socket 建立一条连接、写一行、读回一个编号后关闭（`transport::mark_at`，上限 250 毫秒）。它没有新增 listener、进程、线程、定时器或 `~` 下的文件，走的是唯一的那条 IPC 路径；但“Deck 自己连接自己的 socket”是此前没有的行为。执行者认为它在 EDR-quiet 契约之内（本地 UNIX socket、同一进程、无进程面变化），并已在 `tests/edr_quiet.rs` 头部“Local IPC”一条里如实写明这一事实（未改动该文件的任何检查）；是否接受这一新行为请裁决。
3. **wire v2 的新元素**：`hello`（无声明）、`attest`、每个应答里的 `nonce`、`again`、拒绝码 `no-challenge` 与 `sender-mismatch`；`capabilities` 独立行与结束词 `eof` 取消。每帧多带 `lease` 与 `nonce`。Deck 自己的一行与它的应答（`place`）不属于 observer 的 wire。
4. **F03 的告知时刻**：Deck 仍在开关变化时立即撤回；observer 改为在下一条消息的应答里得知（绑定时在 `accepted` 里，因此“先收紧再 accepted”的效果保留；空闲时最迟一个心跳）。期间至多有一帧在途并被回答 `revoked`。native 两轮 `capabilities` 中这样的在途帧实测为 0 帧。
5. **发送者未被证明的坏行不再结束 lease。** 上一轮“任何坏帧都结束 stream”；现在只有当行能读出 lease 与 nonce、且发送者被证明时，坏帧才结束该 lease（“一个被证明的 observer 说了不该说的”）；否则任何人都不受影响。这是 Follow-up “无法证明来源时不得改变状态”的直接结果，但改变了上一轮测试的一条期望。
6. **同一进程里的第二个 observer 在被告知 `duplicate-observer` 持续 20 秒后自行停止**（原为无限重试）。在 `f37cdd2` 之后它不再与安全相关（§3.6 不依赖 observer 的连接数），只是避免一个永远绑定不上的实例无休止地重试。
7. **`late`**：消息交给 worker 后 2 秒内若还不能写入，则不写入并结束该 lease（上一轮的同名规则，原先由每连接的任务计时，现在由 worker 在写入前检查）。
8. **Lease 结束后保留 60 秒的“墓碑”**（至多 64 个），使迟到的帧被告知原因（`stale`、`capability-disabled` 等）而不是笼统的 `late`；`capability-disabled` 由此成为 observer 得知“须停止直到 reload”的途径。
9. **设计 README 未改**：它是负责人的文档；上述偏差请裁决后由负责人决定是否回写。

## 14. 残余风险（如实列出）

1. **“建立一条连接是一步”这个前提。** §3.6 的 (c) 给出的是“这条连接的 `connect()` 完成于查询之后”。要得到“持有它的子进程 fork 于查询之后”，还需要它的 `socket()` 也在查询之后。两种运行时都在一次原生调用里先后执行 `socket()` 与 `connect()`（Node 经 libuv 的 `uv_pipe_connect`；Bun 经 uSockets 的 unix connect），JS 线程自己无法在其间 fork；但若同一进程的**另一线程**恰在这两个系统调用之间 fork（例如某个 worker 线程在那一刻创建子进程），并且 Deck 的一次查询也恰落在这两个系统调用之间，那么该子进程持有一条被判为“较晚”的连接而未被计数。要构成冒名，它还必须得到 lease 当前的 nonce——那只出现在写给 P 自己连接的应答里——并赢下与父进程自身写入之间的竞态。执行者没有找到在公开原语内消除这一前提的办法（内核不提供连接的创建时刻），如实保留。Bun 是否只在 JS 线程上创建子进程未经验证。
2. **进程主动交出连接、nonce 或字节，或其内存可被读取**（调试器权限、恶意的进程内扩展）：等同于该进程自己说话，不在证明范围内。**启动该进程的父辈**同样不被区分：它本可以自己充当 pane 的前台程序。绑定证明的是“前台程序这样说”，不是代码身份（P3 因此 BLOCKED）。
3. **所依赖的内核语义没有文档保证**：`LOCAL_PEERPID` 的“最后使用者”语义、fork 报告先于子进程运行、`accept` 按建立次序交出连接。三者都有源码与本机实验依据，并各有真实内核测试在语义改变时会失败（`r01_…` 的第 1 类次序同时依赖前两条：内核须把父进程报告为 peer，且 `again` 的原因须恰为 `fork`；第三条由 `a_connection_s_place_is_the_order_in_which_it_was_made` 与 `r01_a_connection_made_before_…` 覆盖）。只在 macOS 27.0.1 上验证。
4. **worker 取 mark 时是阻塞的**（上限 250 毫秒，正常为亚毫秒）：其间排在后面的 Pi 消息等待。listener 的首行名额或 dispatcher 队列被占满时取不到 mark，相应消息被答复 `busy`，observer 重试若干次后放弃 lease 并按退避重新绑定：覆盖降级，不产生假状态。此前在同样的拥塞下 Pi 消息本身也进不来。
5. **告别失败时 lease 最多滞留 15 秒**：observer 放弃了 lease 而 Deck 没收到告别（例如 Deck 自身停顿）时，卡片在这段时间里仍显示该 lease 的最后一个词，随后撤回；这期间重新绑定被拒 `duplicate-observer`。上一轮由连接断开即时结束。
6. **fork 比重试更密的 Pi** 会失去 lease 并按退避重新绑定：覆盖降级，不产生假状态。真实 Pi 的工具调用实测每次只需 1 次重试。
7. **v1 runtime 升级后** 不显示 “needs reload”（§7）。
8. **20 秒的重复判定用墙上时钟**：时钟跳变可能让第二个 observer 提前或推迟停止（仅在同一 Pi 加载了两个 Deck 入口时；与安全无关）。

`c5aa34e`…`ad2b0fd` 草稿里列为第 1 条的“同一 Pi 进程里出现两个 observer 实例”不再是残余：`module-instances/` 的测量（两个不同的 Deck 入口文件各自得到独立的模块实例）仍然成立，但 §3.6 不再以连接数为前提——任何在查询之前建立的连接，不论属于哪个实例，都不被采信。

## 15. 未运行与阻塞

| 项 | 状态 | 说明 |
| --- | --- | --- |
| 完整 P1（needs-input） | BLOCKED | 上游无可靠原语 |
| P3（re-entry） | BLOCKED | 供应方运行代码身份未被证明；拒绝测试通过不等于 PASS |
| 通知发布（away notification） | UNRUN | ad-hoc 载体不能发布通知 |
| 带账号的真实 Claude/Codex 与 Pi 并存 | UNRUN | 本轮不使用任何账号 |
| 已签名的 release 载体 | UNRUN | 未授权 release；真实窗口用本地 debug 载体（ad-hoc 签名） |
| macOS 14（`test-compat-candidate`）及其它系统版本 | UNRUN | 本机与测试机均为 27.0.1 |
| Pi 1.1.0 以外的版本 | UNRUN | |
| §4.2 的两个构造在真实 Pi 进程里（而不是 C fixture 里） | UNRUN | 它们需要 Pi 的子进程是懂协议的敌意纯 fork 副本；只在 C fixture + 真实内核 + 生产 listener 上验证 |

## 16. 本目录的文件

| 文件 | 内容 |
| --- | --- |
| `primitive/` | 四个原语实验的程序与原始输出（§3.1–§3.4），以及所引内核源文件的来源与哈希（`xnu-sources.txt`） |
| `runtime-close/` | 两种运行时上 `destroy()` 关闭描述符的测量（§3.6；不再是证明的前提） |
| `module-instances/` | Pi 加载两个入口文件时模块实例是否共享的测量（§14 末） |
| `pinned-v1/` | 在被复验代码上钉住的两条失败测试及其输出（§12 第 1 行） |
| `pinned-v2/` | 在 `ad2b0fd` 上钉住的两条失败测试、当时的测试 diff 及其输出（§4.2、§12 第 11 行） |
| `gates.json` | 门禁逐步结果 |
| `native-result.json`、`native-scenarios.json` | native 矩阵的摘要（不含 native session id）与逐场景计数 |
| `wk-*.json`、`wk-*.verdict.txt`、`wk-*.applog.txt`、`snapshots.json`、`shots/` | 三次真实窗口运行 |
