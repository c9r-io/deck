// only.c — TEST ONLY. A Node-API addon (loads in Node and in Bun) that lets
// the calling thread be the ONLY thread of its process that can run, for as
// long as a test says so.
//
//   self()     -> { tid, main }        the calling thread (pthread_threadid_np)
//   threads()  -> [{ tid, name, user_us, system_us, user_ns, system_ns, suspend, state, at }]
//                                      `at` is the thread's program counter and stack pointer
//   hold()     -> n                    suspend every OTHER thread of the task
//   release()  -> n                    resume what hold() suspended
//   watchForks() -> bool               from now on, note every thread that calls fork()
//   forks()    -> [{ tid, main, name }]  the calls noted since the last reading
//
// hold() uses the Mach call a debugger uses, on the process's own threads
// only (task_threads on mach_task_self): no other process is touched, no
// environment variable or injected library is involved, and nothing is
// hooked. A suspended thread executes no user-level instruction, so it
// cannot enter a system call: what the kernel does for this process while
// the others are held, the calling thread asked for.
//
// watchForks() registers a pthread_atfork "prepare" handler, the C library's
// own documented callback: the library runs it ON THE THREAD THAT CALLS
// fork(), before the fork. It says nothing of posix_spawn, which is no fork.
#include <node_api.h>

#include <mach/mach.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MAX_HELD 512
static thread_act_t held[MAX_HELD];
static mach_msg_type_number_t held_count = 0;

#define MAX_FORKS 4096
static struct {
  uint64_t tid;
  int main;
  char name[64];
} fork_log[MAX_FORKS];
static atomic_int fork_seen = 0;
static int fork_watch = 0;

// runs on the thread that called fork(), in the parent, before the fork
static void before_fork(void) {
  int at = atomic_fetch_add(&fork_seen, 1);
  if (at >= MAX_FORKS) return;
  pthread_threadid_np(NULL, &fork_log[at].tid);
  fork_log[at].main = pthread_main_np() != 0;
  fork_log[at].name[0] = 0;
  pthread_getname_np(pthread_self(), fork_log[at].name, sizeof fork_log[at].name);
}

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

static double micros(time_value_t t) { return (double)t.seconds * 1e6 + (double)t.microseconds; }

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
      set(env, row, "user_us", number(env, micros(basic.user_time)));
      set(env, row, "system_us", number(env, micros(basic.system_time)));
      set(env, row, "suspend", number(env, basic.suspend_count));
      set(env, row, "state", number(env, basic.run_state));
    }
    thread_extended_info_data_t ext;
    n = THREAD_EXTENDED_INFO_COUNT;
    if (thread_info(list[i], THREAD_EXTENDED_INFO, (thread_info_t)&ext, &n) == KERN_SUCCESS) {
      napi_value name;
      napi_create_string_utf8(env, ext.pth_name, strnlen(ext.pth_name, sizeof ext.pth_name), &name);
      set(env, row, "name", name);
      // the kernel's own counters, in nanoseconds
      set(env, row, "user_ns", number(env, (double)ext.pth_user_time));
      set(env, row, "system_ns", number(env, (double)ext.pth_system_time));
    }
    // Where the thread stands in its own code. Of a held thread this does not
    // change for as long as it executes no user-level instruction.
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
    napi_set_element(env, out, i, row);
    mach_port_deallocate(mach_task_self(), list[i]);
  }
  vm_deallocate(mach_task_self(), (vm_address_t)list, count * sizeof(thread_act_t));
  return out;
}

// Suspend every other thread. Returns how many were suspended, or -1 when a
// hold is already in place or the kernel refused one (then none stays held).
static napi_value hold(napi_env env, napi_callback_info info) {
  (void)info;
  if (held_count != 0) return number(env, -1);
  thread_act_array_t list = NULL;
  mach_msg_type_number_t count = 0;
  if (task_threads(mach_task_self(), &list, &count) != KERN_SUCCESS) return number(env, -1);
  thread_act_t me = mach_thread_self();
  int failed = 0;
  for (mach_msg_type_number_t i = 0; i < count; i++) {
    if (list[i] == me || held_count >= MAX_HELD || failed) {
      mach_port_deallocate(mach_task_self(), list[i]);
      continue;
    }
    if (thread_suspend(list[i]) == KERN_SUCCESS) {
      held[held_count++] = list[i];
    } else {
      // a thread that ended between the listing and here is no failure
      thread_basic_info_data_t basic;
      mach_msg_type_number_t n = THREAD_BASIC_INFO_COUNT;
      if (thread_info(list[i], THREAD_BASIC_INFO, (thread_info_t)&basic, &n) == KERN_SUCCESS) failed = 1;
      mach_port_deallocate(mach_task_self(), list[i]);
    }
  }
  mach_port_deallocate(mach_task_self(), me);
  vm_deallocate(mach_task_self(), (vm_address_t)list, count * sizeof(thread_act_t));
  if (failed) {
    for (mach_msg_type_number_t i = 0; i < held_count; i++) {
      thread_resume(held[i]);
      mach_port_deallocate(mach_task_self(), held[i]);
    }
    held_count = 0;
    return number(env, -1);
  }
  return number(env, held_count);
}

static napi_value release(napi_env env, napi_callback_info info) {
  (void)info;
  mach_msg_type_number_t n = held_count;
  for (mach_msg_type_number_t i = 0; i < held_count; i++) {
    thread_resume(held[i]);
    mach_port_deallocate(mach_task_self(), held[i]);
  }
  held_count = 0;
  return number(env, n);
}

static napi_value watch_forks(napi_env env, napi_callback_info info) {
  (void)info;
  if (!fork_watch) fork_watch = pthread_atfork(before_fork, NULL, NULL) == 0;
  napi_value out;
  napi_get_boolean(env, fork_watch != 0, &out);
  return out;
}

static napi_value forks(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value out;
  napi_create_array(env, &out);
  int seen = atomic_exchange(&fork_seen, 0);
  for (int i = 0; i < seen && i < MAX_FORKS; i++) {
    napi_value row, is_main, name;
    napi_create_object(env, &row);
    napi_get_boolean(env, fork_log[i].main != 0, &is_main);
    napi_create_string_utf8(env, fork_log[i].name, strnlen(fork_log[i].name, sizeof fork_log[i].name), &name);
    set(env, row, "tid", number(env, (double)fork_log[i].tid));
    set(env, row, "main", is_main);
    set(env, row, "name", name);
    napi_set_element(env, out, (uint32_t)i, row);
  }
  return out;
}

NAPI_MODULE_INIT() {
  napi_property_descriptor props[] = {
      {"self", NULL, self, NULL, NULL, NULL, napi_default, NULL},
      {"threads", NULL, threads, NULL, NULL, NULL, napi_default, NULL},
      {"hold", NULL, hold, NULL, NULL, NULL, napi_default, NULL},
      {"release", NULL, release, NULL, NULL, NULL, napi_default, NULL},
      {"watchForks", NULL, watch_forks, NULL, NULL, NULL, napi_default, NULL},
      {"forks", NULL, forks, NULL, NULL, NULL, napi_default, NULL},
  };
  napi_define_properties(env, exports, sizeof props / sizeof props[0], props);
  return exports;
}
