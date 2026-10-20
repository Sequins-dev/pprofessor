/* Final pprof checkpoint handshake, independent of Apple's allocator logger.
 * Unlike leaks --atExit, forked children never wait for the parent's profiler.
 * The protocol stays internal; public artifacts are only pprof snapshots. */
#include <errno.h>
#define INTERPOSE(replacement_fn, original_fn)                                 \
  __attribute__((used, section("__DATA,__interpose"))) static const struct {   \
    const void *replacement;                                                   \
    const void *original;                                                      \
  } interpose_##original_fn = {(const void *)&replacement_fn,                  \
                               (const void *)&original_fn};
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

static int channel = -1;
static pid_t owner;
static _Atomic int finishing;
static void finish(void) {
  if (getpid() != owner || atomic_exchange(&finishing, 1) || channel < 0)
    return;
  char event = 'F';
  if (send(channel, &event, 1, 0) != 1)
    return;
  char reply;
  while (read(channel, &reply, 1) < 0 && errno == EINTR) {
  }
  close(channel);
  channel = -1;
}
__attribute__((constructor)) static void begin(void) {
  const char *parent = getenv("PPROFESSOR_HEAP_PARENT");
  const char *fd = getenv("PPROFESSOR_HEAP_CHANNEL");
  if (!parent || !fd || getppid() != (pid_t)strtol(parent, NULL, 10))
    return;
  owner = getpid();
  channel = (int)strtol(fd, NULL, 10);
  int no_sigpipe = 1;
  setsockopt(channel, SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe,
             sizeof(no_sigpipe));
  char event = 'S';
  send(channel, &event, 1, 0);
}
__attribute__((destructor)) static void normal_exit(void) { finish(); }
__attribute__((noreturn)) static void immediate_exit(int status) {
  finish();
  _exit(status);
}
__attribute__((noreturn)) static void immediate_Exit(int status) {
  finish();
  _Exit(status);
}
INTERPOSE(immediate_exit, _exit)
INTERPOSE(immediate_Exit, _Exit)
