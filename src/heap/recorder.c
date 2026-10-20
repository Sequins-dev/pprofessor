/* Native glibc allocation observation. Exact, ordered events; no allocator
 * replacement, sampling estimates, private malloc hooks, or target IPC thread.
 * The recording path uses initial-exec TLS and a recursion guard. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <execinfo.h>
#include <fcntl.h>
#include <limits.h>
#include <malloc.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

/* glibc's implementation entry points avoid dlsym's bootstrap allocations.
 * This backend intentionally supports glibc's system allocator only. */
extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void __libc_free(void *);
extern void *__libc_memalign(size_t, size_t);
static __thread int inside __attribute__((tls_model("initial-exec")));
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static _Atomic int fd = -1;
static pid_t owner;
static void *(*aligned_impl)(size_t, size_t);
static uint64_t *header;
static char map_path[PATH_MAX];
struct event {
  uint64_t kind, address, old_address, size, depth, pcs[64];
};
static int write_all(int out, const void *buf, size_t size) {
  const char *p = buf;
  while (size) {
    ssize_t n = write(out, p, size);
    if (n < 0 && errno == EINTR)
      continue;
    if (n <= 0)
      return -1;
    p += n;
    size -= (size_t)n;
  }
  return 0;
}
static void save_maps(void) {
  int in = open("/proc/self/maps", O_RDONLY | O_CLOEXEC);
  int out = open(map_path,
                 O_CREAT | O_WRONLY | O_TRUNC | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (in >= 0 && out >= 0) {
    char buf[4096];
    ssize_t n;
    while ((n = read(in, buf, sizeof(buf))) > 0)
      if (write_all(out, buf, (size_t)n))
        break;
  }
  if (in >= 0)
    close(in);
  if (out >= 0)
    close(out);
}
__attribute__((noinline)) static void record(uint64_t kind, void *p, void *old,
                                             size_t size) {
  if (fd < 0)
    return;
  struct event e = {0};
  e.kind = kind;
  e.address = (uintptr_t)p;
  e.old_address = (uintptr_t)old;
  e.size = size;
  if (kind == 1 || kind == 3) {
    void *pcs[64];
    int n = backtrace(pcs, 64);
    for (int i = 2; i < n; i++)
      e.pcs[e.depth++] = (uintptr_t)pcs[i];
  }
  if (write_all(fd, &e, sizeof(e)))
    __atomic_store_n(&header[3], 1, __ATOMIC_RELEASE);
}
static int enter(void) {
  if (inside || fd < 0 || getpid() != owner)
    return 0;
  inside = 1;
  pthread_mutex_lock(&lock);
  return 1;
}
static void leave(void) {
  pthread_mutex_unlock(&lock);
  inside = 0;
}
static void before_fork(void) {
  if (!inside)
    pthread_mutex_lock(&lock);
}
static void after_fork(void) {
  if (!inside)
    pthread_mutex_unlock(&lock);
}
static void fork_child(void) {
  if (fd >= 0)
    close(fd);
  fd = -1;
  header = NULL;
  inside = 0;
  pthread_mutex_t fresh = PTHREAD_MUTEX_INITIALIZER;
  lock = fresh;
}
__attribute__((constructor)) static void begin(void) {
  inside = 1;
  const char *path = getenv("PPROFESSOR_HEAP_LOG");
  if (!path || strlen(path) > PATH_MAX - 6) {
    inside = 0;
    return;
  }
  aligned_impl = (void *(*)(size_t, size_t))dlsym(RTLD_NEXT, "aligned_alloc");
  owner = getpid();
  fd = open(path, O_CREAT | O_EXCL | O_RDWR | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (fd < 0) {
    // A same-PID exec replaces every allocation without emitting frees. Mark
    // the previous stream incomplete instead of returning stale live bytes.
    int existing = open(path, O_RDWR | O_CLOEXEC | O_NOFOLLOW);
    if (existing >= 0) {
      uint64_t previous[4];
      if (pread(existing, previous, sizeof(previous), 0) == sizeof(previous) &&
          previous[0] == 0x3130504145485050ULL &&
          previous[2] == (uint64_t)owner) {
        uint64_t replaced = 3;
        (void)pwrite(existing, &replaced, sizeof(replaced), 24);
      }
      close(existing);
    }
    inside = 0;
    return;
  }
  uint64_t h[4] = {0x3130504145485050ULL, 1, (uint64_t)owner, 0}; /* PPHEAP01 */
  if (write_all(fd, h, sizeof(h))) {
    close(fd);
    fd = -1;
    inside = 0;
    return;
  }
  header = mmap(NULL, 32, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (header == MAP_FAILED) {
    close(fd);
    fd = -1;
    header = NULL;
    inside = 0;
    return;
  }
  strcpy(map_path, path);
  strcat(map_path, ".maps");
  save_maps();
  void *pcs[64];
  backtrace(pcs, 64); /* load unwind support outside observation */
  pthread_atfork(before_fork, after_fork, fork_child);
  inside = 0;
}
__attribute__((destructor)) static void finish(void) {
  if (fd < 0 || getpid() != owner)
    return;
  inside = 1;
  pthread_mutex_lock(&lock);
  save_maps();
  __atomic_store_n(&header[3],
                   __atomic_load_n(&header[3], __ATOMIC_ACQUIRE) ? 1 : 2,
                   __ATOMIC_RELEASE);
  close(fd);
  fd = -1;
  pthread_mutex_unlock(&lock);
}
void *malloc(size_t size) {
  int observe = enter();
  void *p = __libc_malloc(size);
  int saved = errno;
  if (observe) {
    if (p)
      record(1, p, NULL, malloc_usable_size(p));
    leave();
  }
  errno = saved;
  return p;
}
void *calloc(size_t n, size_t size) {
  int observe = enter();
  void *p = __libc_calloc(n, size);
  int saved = errno;
  if (observe) {
    if (p)
      record(1, p, NULL, malloc_usable_size(p));
    leave();
  }
  errno = saved;
  return p;
}
void free(void *p) {
  int saved = errno;
  int observe = enter();
  if (observe && p)
    record(2, p, NULL, 0);
  __libc_free(p);
  if (observe)
    leave();
  errno = saved;
}
void *realloc(void *old, size_t size) {
  int observe = enter();
  void *p = __libc_realloc(old, size);
  int saved = errno;
  if (observe) {
    if (p || size == 0)
      record(3, p, old, p ? malloc_usable_size(p) : 0);
    leave();
  }
  errno = saved;
  return p;
}
void *reallocarray(void *p, size_t n, size_t size) {
  if (size && n > SIZE_MAX / size) {
    errno = ENOMEM;
    return NULL;
  }
  return realloc(p, n * size);
}
void *memalign(size_t alignment, size_t size) {
  int observe = enter();
  void *p = __libc_memalign(alignment, size);
  int saved = errno;
  if (observe) {
    if (p)
      record(1, p, NULL, malloc_usable_size(p));
    leave();
  }
  errno = saved;
  return p;
}
void *aligned_alloc(size_t alignment, size_t size) {
  // glibc changed invalid-alignment handling between releases. Forward the
  // public API rather than approximating it with memalign.
  if (!aligned_impl)
    return memalign(alignment, size);
  int observe = enter();
  void *p = aligned_impl(alignment, size);
  int saved = errno;
  if (observe) {
    if (p)
      record(1, p, NULL, malloc_usable_size(p));
    leave();
  }
  errno = saved;
  return p;
}
int posix_memalign(void **out, size_t alignment, size_t size) {
  if (alignment < sizeof(void *) || (alignment & (alignment - 1)))
    return EINVAL;
  int saved = errno;
  void *p = memalign(alignment, size);
  int result = p ? 0 : ENOMEM;
  if (p)
    *out = p;
  errno = saved;
  return result;
}
void *valloc(size_t size) {
  return memalign((size_t)sysconf(_SC_PAGESIZE), size);
}
void *pvalloc(size_t size) {
  size_t page = (size_t)sysconf(_SC_PAGESIZE);
  if (size > SIZE_MAX - (page - 1)) {
    errno = ENOMEM;
    return NULL;
  }
  return memalign(page, (size + page - 1) & ~(page - 1));
}
void cfree(void *p) { free(p); }
