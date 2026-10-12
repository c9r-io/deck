// only2.c — TEST ONLY. only.c, and a WITNESS: one thread of this addon's own
// that a hold never suspends.
//
//   self()     -> { tid, main }
//   threads()  -> [{ tid, name, user_ns, system_ns, suspend, state, at, witness }]
//   hold()     -> n                    suspend every OTHER thread of the task (but the witness)
//   release()  -> n                    resume what hold() suspended
//   witness(ms, file) -> bool          start the witness; it acts on a hold older than `ms`
//   rescued()  -> n                    how many holds the witness has ended so far
//   written()  -> n                    how many of them it has written down
//
// What the witness is for. A held thread may hold something the holding
// thread needs; then the holder waits for ever and nothing can be learned
// from the process any more. The witness makes that case OBSERVABLE: when a
// hold has lasted longer than `ms` it
//   1. reads, of every thread, where it stands (program counter, the chain of
//      return addresses, the first registers) and what the kernel says of it —
//      of the holder twice, 200 ms apart, with the processor time it was
//      charged in between;
//   2. resumes the held threads itself;
//   3. writes what it read as one JSON line to `file`.
// Whether the holder then goes on by itself is the test's own answer to
// "was it the hold": the JavaScript that called hold() reaches release() only
// if what it waited for was something a held thread had.
//
// While the others are held the witness calls nothing that takes a lock of
// the process: Mach calls on its own task, memset and memcpy. Names of
// functions are looked up (dladdr) and the line is formatted only after the
// others run again.
#include <node_api.h>

#include <dlfcn.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <sched.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define MAX_HELD 512
static thread_act_t held[MAX_HELD];
static mach_msg_type_number_t held_count = 0;

// 0: no hold · 1: a hold is in place · 2: the witness is reading · 3: the witness resumed the held threads
static atomic_int phase = 0;
static _Atomic uint64_t held_at = 0;  // mach_absolute_time() when the hold was complete
static thread_act_t holder = 0;       // the thread that holds
static thread_act_t witness_port = 0;
static atomic_int witness_ready = 0;
static atomic_int rescues = 0;
static atomic_int told = 0;  // how many of them are written down
static int witness_fd = -1;
static uint64_t patience = 0;         // in units of mach_absolute_time()
static mach_timebase_info_data_t timebase;

static uint64_t nanos(uint64_t ticks) { return ticks * timebase.numer / timebase.denom; }

static napi_value number(napi_env env, double value) {
  napi_value out;
  napi_create_double(env, value, &out);
  return out;
}

static void set(napi_env env, napi_value object, const char *key, napi_value value) {
  napi_set_named_property(env, object, key, value);
}

static napi_value self(napi_env env, napi_callback_info info) {
  (void)info;
  uint64_t tid = 0;
  pthread_threadid_np(NULL, &tid);
  napi_value out, is_main;
  napi_create_object(env, &out);
  napi_get_boolean(env, pthread_main_np() != 0, &is_main);
  set(env, out, "tid", number(env, (double)tid));
  set(env, out, "main", is_main);
  return out;
}

static napi_value threads(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value out;
  napi_create_array(env, &out);
  thread_act_array_t list = NULL;
  mach_msg_type_number_t count = 0;
  if (task_threads(mach_task_self(), &list, &count) != KERN_SUCCESS) return out;
  for (mach_msg_type_number_t i = 0; i < count; i++) {
    napi_value row;
    napi_create_object(env, &row);
    thread_identifier_info_data_t id;
    mach_msg_type_number_t n = THREAD_IDENTIFIER_INFO_COUNT;
    if (thread_info(list[i], THREAD_IDENTIFIER_INFO, (thread_info_t)&id, &n) == KERN_SUCCESS)
      set(env, row, "tid", number(env, (double)id.thread_id));
    thread_basic_info_data_t basic;
    n = THREAD_BASIC_INFO_COUNT;
    if (thread_info(list[i], THREAD_BASIC_INFO, (thread_info_t)&basic, &n) == KERN_SUCCESS) {
      set(env, row, "suspend", number(env, basic.suspend_count));
      set(env, row, "state", number(env, basic.run_state));
    }
    thread_extended_info_data_t ext;
    n = THREAD_EXTENDED_INFO_COUNT;
    if (thread_info(list[i], THREAD_EXTENDED_INFO, (thread_info_t)&ext, &n) == KERN_SUCCESS) {
      napi_value name;
      napi_create_string_utf8(env, ext.pth_name, strnlen(ext.pth_name, sizeof ext.pth_name), &name);
      set(env, row, "name", name);
      set(env, row, "user_ns", number(env, (double)ext.pth_user_time));
      set(env, row, "system_ns", number(env, (double)ext.pth_system_time));
    }
    arm_thread_state64_t regs;
    n = ARM_THREAD_STATE64_COUNT;
    if (thread_get_state(list[i], ARM_THREAD_STATE64, (thread_state_t)&regs, &n) == KERN_SUCCESS) {
      char at[48];
      snprintf(at, sizeof at, "%llx:%llx", (unsigned long long)arm_thread_state64_get_pc(regs),
               (unsigned long long)arm_thread_state64_get_sp(regs));
      napi_value where;
      napi_create_string_utf8(env, at, NAPI_AUTO_LENGTH, &where);
      set(env, row, "at", where);
    }
    napi_value is_witness;
    napi_get_boolean(env, witness_port != 0 && list[i] == witness_port, &is_witness);
    set(env, row, "witness", is_witness);
    napi_set_element(env, out, i, row);
    mach_port_deallocate(mach_task_self(), list[i]);
  }
  vm_deallocate(mach_task_self(), (vm_address_t)list, count * sizeof(thread_act_t));
  return out;
}

// Suspend every other thread but the witness. Returns how many were suspended,
// or -1 when a hold is already in place or the kernel refused one.
static napi_value hold(napi_env env, napi_callback_info info) {
  (void)info;
  if (held_count != 0 || atomic_load(&phase) != 0) return number(env, -1);
  thread_act_array_t list = NULL;
  mach_msg_type_number_t count = 0;
  if (task_threads(mach_task_self(), &list, &count) != KERN_SUCCESS) return number(env, -1);
  thread_act_t me = mach_thread_self();
  int failed = 0;
  for (mach_msg_type_number_t i = 0; i < count; i++) {
    if (list[i] == me || list[i] == witness_port || held_count >= MAX_HELD || failed) {
      mach_port_deallocate(mach_task_self(), list[i]);
      continue;
    }
    if (thread_suspend(list[i]) == KERN_SUCCESS) {
      held[held_count++] = list[i];
    } else {
      thread_basic_info_data_t basic;
      mach_msg_type_number_t n = THREAD_BASIC_INFO_COUNT;
      if (thread_info(list[i], THREAD_BASIC_INFO, (thread_info_t)&basic, &n) == KERN_SUCCESS) failed = 1;
      mach_port_deallocate(mach_task_self(), list[i]);
    }
  }
  vm_deallocate(mach_task_self(), (vm_address_t)list, count * sizeof(thread_act_t));
  if (failed) {
    for (mach_msg_type_number_t i = 0; i < held_count; i++) {
      thread_resume(held[i]);
      mach_port_deallocate(mach_task_self(), held[i]);
    }
    held_count = 0;
    mach_port_deallocate(mach_task_self(), me);
    return number(env, -1);
  }
  holder = me;
  atomic_store(&held_at, mach_absolute_time());
  atomic_store(&phase, 1);
  return number(env, held_count);
}

static napi_value release(napi_env env, napi_callback_info info) {
  (void)info;
  mach_msg_type_number_t n = held_count;
  if (atomic_load(&phase) == 0) return number(env, 0);
  for (;;) {
    int was = 1;
    if (atomic_compare_exchange_strong(&phase, &was, 0)) {
      for (mach_msg_type_number_t i = 0; i < held_count; i++) thread_resume(held[i]);
      break;
    }
    if (was == 3) {
      // the witness resumed them
      atomic_store(&phase, 0);
      break;
    }
    sched_yield();  // the witness is reading
  }
  for (mach_msg_type_number_t i = 0; i < held_count; i++) mach_port_deallocate(mach_task_self(), held[i]);
  held_count = 0;
  if (holder) mach_port_deallocate(mach_task_self(), holder);
  holder = 0;
  return number(env, n);
}

// ---------------------------------------------------------------- the witness

#define MAX_SEEN 96
#define MAX_FRAMES 56
struct seen {
  thread_act_t port;
  uint64_t tid;
  char name[64];
  int is_holder, is_held, run_state, flags, suspend;
  uint64_t user_ns, system_ns;
  uint64_t pc, lr, fp, sp, x[8], x16;
  int have_word;
  uint64_t word;  // what stands where x1 points (a lock's own word, when the thread waits in ulock_wait)
  int frames;
  uint64_t frame[MAX_FRAMES];
};
static struct seen first[MAX_SEEN];
static int first_count = 0;
static struct seen again;  // the holder a second time
static int have_again = 0;
static uint64_t waited_ns = 0;
static char line[400000];
static unsigned told_held = 0;

// a return address as it stands in a frame of a system library carries a signature in its upper bits
static uint64_t plain(uint64_t address) { return address & 0x00007fffffffffffULL; }

static int peek(uint64_t address, void *into, size_t size) {
  mach_vm_size_t got = 0;
  return mach_vm_read_overwrite(mach_task_self(), address, size, (mach_vm_address_t)into, &got) == KERN_SUCCESS && got == size;
}

static void read_thread(thread_act_t thread, struct seen *s) {
  memset(s, 0, sizeof *s);
  s->port = thread;
  thread_identifier_info_data_t id;
  mach_msg_type_number_t n = THREAD_IDENTIFIER_INFO_COUNT;
  if (thread_info(thread, THREAD_IDENTIFIER_INFO, (thread_info_t)&id, &n) == KERN_SUCCESS) s->tid = id.thread_id;
  thread_basic_info_data_t basic;
  n = THREAD_BASIC_INFO_COUNT;
  if (thread_info(thread, THREAD_BASIC_INFO, (thread_info_t)&basic, &n) == KERN_SUCCESS) {
    s->run_state = basic.run_state;
    s->flags = basic.flags;
    s->suspend = basic.suspend_count;
  }
  thread_extended_info_data_t ext;
  n = THREAD_EXTENDED_INFO_COUNT;
  if (thread_info(thread, THREAD_EXTENDED_INFO, (thread_info_t)&ext, &n) == KERN_SUCCESS) {
    memcpy(s->name, ext.pth_name, sizeof s->name - 1);
    s->user_ns = ext.pth_user_time;
    s->system_ns = ext.pth_system_time;
  }
  arm_thread_state64_t regs;
  n = ARM_THREAD_STATE64_COUNT;
  if (thread_get_state(thread, ARM_THREAD_STATE64, (thread_state_t)&regs, &n) != KERN_SUCCESS) return;
  s->pc = plain(arm_thread_state64_get_pc(regs));
  s->lr = plain(arm_thread_state64_get_lr(regs));
  s->fp = arm_thread_state64_get_fp(regs);
  s->sp = arm_thread_state64_get_sp(regs);
  for (int i = 0; i < 8; i++) s->x[i] = regs.__x[i];
  s->x16 = regs.__x[16];
  s->have_word = peek(s->x[1], &s->word, sizeof s->word);
  s->frame[s->frames++] = s->pc;
  s->frame[s->frames++] = s->lr;
  uint64_t fp = s->fp;
  while (fp && s->frames < MAX_FRAMES) {
    uint64_t pair[2];
    if (!peek(fp, pair, sizeof pair)) break;
    s->frame[s->frames++] = plain(pair[1]);
    if (pair[0] <= fp) break;
    fp = pair[0];
  }
}

// every thread but the witness; the holder is stopped for the length of its own reading
static void look(void) {
  first_count = 0;
  thread_act_array_t list = NULL;
  mach_msg_type_number_t count = 0;
  if (task_threads(mach_task_self(), &list, &count) != KERN_SUCCESS) return;
  for (mach_msg_type_number_t i = 0; i < count; i++) {
    if (list[i] != witness_port && first_count < MAX_SEEN) {
      struct seen *s = &first[first_count++];
      int is_holder = list[i] == holder;
      if (is_holder) thread_suspend(list[i]);
      read_thread(list[i], s);
      if (is_holder) thread_resume(list[i]);
      s->is_holder = is_holder;
      for (mach_msg_type_number_t h = 0; h < held_count; h++)
        if (held[h] == list[i]) s->is_held = 1;
    }
    mach_port_deallocate(mach_task_self(), list[i]);
  }
  vm_deallocate(mach_task_self(), (vm_address_t)list, count * sizeof(thread_act_t));
}

static size_t put(size_t at, const char *format, ...) __attribute__((format(printf, 2, 3)));
static size_t put(size_t at, const char *format, ...) {
  if (at >= sizeof line - 1) return at;
  va_list more;
  va_start(more, format);
  int wrote = vsnprintf(line + at, sizeof line - at, format, more);
  va_end(more);
  if (wrote < 0) return at;
  return at + (size_t)wrote < sizeof line ? at + (size_t)wrote : sizeof line - 1;
}

static size_t put_text(size_t at, const char *text) {
  for (const char *c = text; *c && at < sizeof line - 2; c++) line[at++] = (*c == '"' || *c == '\\' || (unsigned char)*c < 0x20) ? '?' : *c;
  line[at] = 0;
  return at;
}

static size_t put_seen(size_t at, const struct seen *s) {
  at = put(at, "{\"tid\":%llu,\"port\":%u,\"name\":\"", (unsigned long long)s->tid, (unsigned)s->port);
  at = put_text(at, s->name);
  at = put(at, "\",\"holder\":%s,\"held\":%s,\"run_state\":%d,\"flags\":%d,\"suspend\":%d,\"user_ns\":%llu,\"system_ns\":%llu,"
               "\"pc\":\"%llx\",\"sp\":\"%llx\",\"x16\":%lld,\"x\":[",
           s->is_holder ? "true" : "false", s->is_held ? "true" : "false", s->run_state, s->flags, s->suspend,
           (unsigned long long)s->user_ns, (unsigned long long)s->system_ns, (unsigned long long)s->pc,
           (unsigned long long)s->sp, (long long)(int64_t)s->x16);
  for (int i = 0; i < 8; i++) at = put(at, "%s\"%llx\"", i ? "," : "", (unsigned long long)s->x[i]);
  at = put(at, "],\"word_at_x1\":");
  at = s->have_word ? put(at, "\"%llx\"", (unsigned long long)s->word) : put(at, "null");
  at = put(at, ",\"frames\":[");
  for (int i = 0; i < s->frames; i++) {
    Dl_info info;
    uint64_t a = s->frame[i];
    at = put(at, "%s{\"a\":\"%llx\"", i ? "," : "", (unsigned long long)a);
    if (a && dladdr((void *)a, &info) && info.dli_fname) {
      const char *base = strrchr(info.dli_fname, '/');
      at = put(at, ",\"image\":\"");
      at = put_text(at, base ? base + 1 : info.dli_fname);
      at = put(at, "\",\"off\":\"%llx\"", (unsigned long long)(a - (uint64_t)info.dli_fbase));
      if (info.dli_sname) {
        at = put(at, ",\"sym\":\"");
        at = put_text(at, info.dli_sname);
        at = put(at, "\",\"plus\":%llu", (unsigned long long)(a - (uint64_t)info.dli_saddr));
      }
    }
    at = put(at, "}");
  }
  return put(at, "]}");
}

static void tell(int number_of_rescue) {
  size_t at = 0;
  at = put(at, "{\"rescue\":%d,\"pid\":%d,\"hold_lasted_ns\":%llu,\"held\":%u,\"holder_first\":", number_of_rescue, getpid(),
           (unsigned long long)waited_ns, told_held);
  int found = 0;
  for (int i = 0; i < first_count; i++)
    if (first[i].is_holder) {
      at = put_seen(at, &first[i]);
      found = 1;
    }
  if (!found) at = put(at, "null");
  at = put(at, ",\"holder_200ms_later\":");
  at = have_again ? put_seen(at, &again) : put(at, "null");
  at = put(at, ",\"others\":[");
  int some = 0;
  for (int i = 0; i < first_count; i++)
    if (!first[i].is_holder) {
      if (some++) at = put(at, ",");
      at = put_seen(at, &first[i]);
    }
  at = put(at, "]}\n");
  if (witness_fd >= 0) (void)!write(witness_fd, line, at);
  atomic_fetch_add(&told, 1);
}

static void *witness_main(void *unused) {
  (void)unused;
  pthread_setname_np("r1 witness");
  witness_port = mach_thread_self();
  atomic_store(&witness_ready, 1);
  for (;;) {
    usleep(25000);
    if (atomic_load(&phase) != 1) continue;
    uint64_t since = atomic_load(&held_at);
    uint64_t now = mach_absolute_time();
    if (now - since < patience) continue;
    int was = 1;
    if (!atomic_compare_exchange_strong(&phase, &was, 2)) continue;
    if (atomic_load(&held_at) != since) {
      // another hold than the one that was timed
      atomic_store(&phase, 1);
      continue;
    }
    waited_ns = nanos(now - since);
    look();
    usleep(200000);
    have_again = 0;
    if (holder) {
      thread_suspend(holder);
      read_thread(holder, &again);
      thread_resume(holder);
      again.is_holder = 1;
      have_again = 1;
    }
    told_held = held_count;
    for (mach_msg_type_number_t i = 0; i < held_count; i++) thread_resume(held[i]);
    int n = atomic_fetch_add(&rescues, 1) + 1;
    atomic_store(&phase, 3);
    // the others run again: names may be looked up and the line written
    tell(n);
  }
  return NULL;
}

static napi_value witness(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], out;
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  double ms = 0;
  char path[1024];
  size_t length = 0;
  int ok = argc == 2 && napi_get_value_double(env, argv[0], &ms) == napi_ok &&
           napi_get_value_string_utf8(env, argv[1], path, sizeof path, &length) == napi_ok && !atomic_load(&witness_ready);
  if (ok) {
    mach_timebase_info(&timebase);
    patience = (uint64_t)(ms * 1e6) * timebase.denom / timebase.numer;
    witness_fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    pthread_t thread;
    ok = witness_fd >= 0 && pthread_create(&thread, NULL, witness_main, NULL) == 0;
    if (ok) {
      pthread_detach(thread);
      while (!atomic_load(&witness_ready)) usleep(1000);
    }
  }
  napi_get_boolean(env, ok, &out);
  return out;
}

static napi_value rescued(napi_env env, napi_callback_info info) {
  (void)info;
  return number(env, atomic_load(&rescues));
}

static napi_value written(napi_env env, napi_callback_info info) {
  (void)info;
  return number(env, atomic_load(&told));
}

NAPI_MODULE_INIT() {
  napi_property_descriptor props[] = {
      {"self", NULL, self, NULL, NULL, NULL, napi_default, NULL},
      {"threads", NULL, threads, NULL, NULL, NULL, napi_default, NULL},
      {"hold", NULL, hold, NULL, NULL, NULL, napi_default, NULL},
      {"release", NULL, release, NULL, NULL, NULL, napi_default, NULL},
      {"witness", NULL, witness, NULL, NULL, NULL, napi_default, NULL},
      {"rescued", NULL, rescued, NULL, NULL, NULL, napi_default, NULL},
      {"written", NULL, written, NULL, NULL, NULL, napi_default, NULL},
  };
  napi_define_properties(env, exports, sizeof props / sizeof props[0], props);
  return exports;
}
