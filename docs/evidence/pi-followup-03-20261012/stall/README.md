# Pi follow-up 03 · “挂起期间 JS 线程停住”的直接观测：材料清单

执行 Agent 的工作材料（2026-10-12），不是验收结论。两个实验根的原始结果（`out/bun-root/`、`out/node-root/`）不入库，留在执行者本机被忽略的 `reports/pi-followup-03-20261012/stall/out/`。它回答主 README（上一级目录的 `README.md`）“失败记录”一节的问题：残余风险 1 的测量里，JS 线程偶尔在“其它线程全部挂起”的窗口里停住——停在哪里、等的是什么、是谁造成的。

## 做法

`only2.c` 在原 addon（`../measure/only.c`）上加一条**见证线程**：`hold()` 从不挂起它。某次挂起超过 `R1_PATIENCE_MS`（1500 毫秒）时，它

1. 读下每条线程的位置：程序计数器、返回地址链、前几个寄存器、内核记的状态与处理器时间——持有挂起的那条线程读两次，相隔 200 毫秒；
2. 自己恢复被挂起的线程；
3. 把读到的内容写成一行 JSON。

其它线程被挂起期间，见证线程只做对本进程的 Mach 调用和 memset/memcpy，不取进程里的任何锁；查函数名（`dladdr`）与格式化都在恢复之后。等锁的线程在系统调用里，寄存器即调用参数：`ulock_wait` 的 x1 是锁地址，锁字里是持有者的线程端口；`psynch_mutexwait` 的 x3 是持有者的线程号。

“恢复之后持有线程是否自己继续”就是“是不是挂起造成的”的判据：`stall.cjs` 里那一轮的 `stall` 行由持有线程在 `release()` 之后写出，它存在即说明该线程走了下去。

## programs/

| 文件 | 作用 |
| --- | --- |
| `only2.c` | 带见证线程的 addon（TEST ONLY）。 |
| `stall.cjs` | `probe.cjs` 加见证：只改了 `alone()`、两个被测循环（各步的时间戳、`stall` 行）、挂起前的间隔（`R1_PAUSE_MS`，默认 2，与 `probe.cjs` 相同；0 即其第一版），并加了对照项 `witness-control`。 |
| `hunt.py` | 运行器：与 `measure.py` 相同的登记、隔离环境与 6 秒停滞检测；一个进程接一个进程地跑，直到观测到 N 次停住。第 23 行是本仓库的绝对路径（与 `../measure/measure.py` 第 31 行同类），换一台机器要改。 |
| `show.py` | 只读：把见证线程写的行打印成调用栈；Bun 的帧用同一发布的 linker map 命名（见 `symbols.txt`）。 |
| `run-hunts.sh`、`run-hunts-node.sh` | 两条搜寻序列（Bun 根、Node 根各一）。 |

## out/

- `observed-stalls.txt`：观测到的 8 次停住，带函数名（先看这个）。
- `control.txt`：对照——主线程故意等一条被挂起的 Worker。
- `bun-root/`、`node-root/`：两个实验根的原始结果：每次搜寻的合计（`*.hunt.json`）、每个进程的运行记录与结果行、见证线程的原始行（`*.witness.jsonl`）、registry、清理日志。

## 结果

对照（`witness-control`）：Node、Bun 各两次运行、每次 3 轮，12 轮全部在见证线程恢复其它线程后自行继续。见证线程写下 11 行：第一次 Node 运行的第三行在进程退出前没来得及写，此后探针改为等这一行写完再继续（`written()`），第二次两边都是 3 行。

| 搜寻 | 进程 | 挂起轮数 | 停住 | 恢复后自行继续 | 被运行器停止的进程 |
| --- | --- | --- | --- | --- | --- |
| Bun · connect | 60 | 2400 | 2 | 2 | 0 |
| Bun · `child_process.exec` | 50 | 2000 | 2 | 2 | 0 |
| Node · connect，挂起前无间隔 | 14 | 4200 | 4 | 4 | 0 |
| Node · connect，间隔 2 ms | 16 + 1 | 1920 + 120 | 0 | – | 0 |
| Node · `spawn`（管道 + detached） | 14 | 560 | 0 | – | 0 |
| Node · `child_process.exec` | 14 | 560 | 0 | – | 0 |

8 次停住里，持有线程都停在一把锁上、200 毫秒后位置不变、其间处理器时间为 0：

| # | 进程 · 轮 | 停在哪一步 | 等的锁 | 持有者（被挂起） | 谁要这把锁 |
| --- | --- | --- | --- | --- | --- |
| 1 | `h-bun-connect-8` · 18 | 末次 `threads()` 读数 | libpas 的 `os_unfair_lock` | libpas scavenger，在 `pas_thread_local_cache_for_all` | addon 自己的 `threads()` 经 N-API 建字符串时申请内存 |
| 2 | `h-bun-connect-17` · 18 | 末次 `threads()` 读数 | 同上 | libpas scavenger，在 `pas_physical_page_sharing_pool_scavenge` | 同上 |
| 3 | `h-bun-exec-47` · 5 | 被测调用（`cp.exec` 的 JS 部分） | 同上 | libpas scavenger，同上 | 一次普通属性访问建属性表时申请内存 |
| 4 | `h-bun-exec-50` · 11 | 被测调用（`cp.exec` 的 JS 部分） | 同上 | libpas scavenger，同上 | 主线程安装刚编译完的 baseline JIT 代码时申请内存 |
| 5 | `n-node-connect-nopause-3` · 104 | `hold()` 返回之后、首次读数之前的一个 JS 函数入口 | 任务队列的 pthread mutex | V8Worker，停在 `uv_cond_wait` 调 `pthread_cond_wait` 的入口（仍持有该 mutex） | 主线程收尾已完成的 Maglev 编译任务，向工作线程投递任务 |
| 6 | `n-node-connect-nopause-12` · 105 | 被测调用返回之后 | `Heap::RegisterStrongRoots` 的 absl mutex | V8Worker，停在同一函数的临界区内（Maglev 编译中） | 主线程新建一个 Maglev 编译任务 |
| 7 | `n-node-connect-nopause-14` · 66 | 同 #5 | 任务队列的 pthread mutex | V8Worker，同 #5 | 同 #5 |
| 8 | `n-node-connect-nopause-14` · 105 | 同 #5 | job 状态的 absl mutex | V8Worker，停在 `DefaultJobState::DidRunTask` 内 | 同 #5 |

#1–#5、#7 的持有者由锁自身的记录认定（锁字里的线程端口、`psynch_mutexwait` 的持有者线程号）；#6、#8 的 absl mutex 不记录持有者，由被挂起线程的调用栈认定（一条 V8Worker 停在同一临界区内）。

没有做的：Bun 的 `spawn`（管道 + detached）搜寻没有跑（序列在两项 Bun 搜寻完成后由我中止，当时它刚开始重复 Node 的 connect；被打断的那一个尝试 `h-node-connect-2` 没有运行记录，其三个进程随后台任务一起结束，registry 显示 gone）；Node 带 2 ms 间隔的各项没有出现停住，所以最终系列条件下 Node 的停住没有被观测过（最终系列里 Node 本来也没有停住）。

## 重跑

```sh
R=$(mktemp -d /private/tmp/deck-pi-r1b-XXXXXX); mkdir -m 700 $R/home $R/tmp $R/bin $R/out $R/src $R/work
python3 scripts/edr_runtime.py experiment init --root $R
cp <node 24.19.0> $R/bin/node; cp <官方 standalone Pi 1.1.0 的 pi> $R/bin/pi-standalone
cc -O1 -o $R/bin/listen ../measure/listen.c; cc -O1 -o $R/bin/kid ../measure/kid.c
cp programs/* $R/src/
cc -O1 -shared -undefined dynamic_lookup -I<node 源码的 src/> -o $R/bin/only2.node $R/src/only2.c
python3 $R/src/hunt.py $R node witness-control - 3 1 wc-node --until 99      # 对照
python3 $R/src/hunt.py $R bun connect - 40 60 h-bun-connect --until 4
python3 $R/src/hunt.py $R node connect - 300 16 n-node-connect-nopause --until 3 R1_PAUSE_MS=0
python3 $R/src/show.py --map <bun-profile.linker-map> --others 0 $R/out/<name>.witness.jsonl
python3 scripts/edr_runtime.py experiment cleanup --root $R
```
