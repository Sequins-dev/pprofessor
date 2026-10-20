#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifdef __linux__
#include <malloc.h>
#endif
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
static void *retained;
__attribute__((noinline)) void heap_retained(void) {
  retained = malloc(1048576);
  ((volatile char *)retained)[0] = 42;
}
__attribute__((noinline)) void heap_churn(void) {
  void *p = malloc(65536);
  ((volatile char *)p)[0] = 42;
  free(p);
}
__attribute__((noinline)) void heap_resize(void) {
  void *p = calloc(1, 8192);
  p = realloc(p, 32768);
  p = realloc(p, 16384);
  ((volatile char *)p)[0] = 42;
  volatile size_t too_large = SIZE_MAX;
  void *failed = realloc(p, too_large);
  if (failed)
    abort();
  free(p);
}
__attribute__((noinline)) void heap_aligned(void) {
  void *p = NULL;
  if (posix_memalign(&p, 4096, 16384))
    abort();
  ((volatile char *)p)[0] = 42;
  free(p);
}
static void *thread_allocation;
__attribute__((noinline)) void heap_cross_thread_allocate(void) {
  thread_allocation = malloc(262144);
}
static void *release_from_thread(void *unused) {
  (void)unused;
  free(thread_allocation);
  return NULL;
}
__attribute__((noinline)) void heap_fork_child(void) {
  void *p = malloc(524288);
  ((volatile char *)p)[0] = 1;
  free(p);
}
int main(int argc, char **argv) {
  alarm(30);
  heap_retained();
  heap_churn();
  heap_resize();
  heap_aligned();
  heap_cross_thread_allocate();
  pthread_t thread;
  if (pthread_create(&thread, NULL, release_from_thread, NULL))
    abort();
  pthread_join(thread, NULL);
  pid_t child = fork();
  if (child == 0) {
    heap_fork_child();
    _exit(0);
  }
  if (child < 0 || waitpid(child, NULL, 0) != child)
    abort();
  if (argc > 1 && strcmp(argv[1], "exec") == 0) {
    execl(argv[0], argv[0], "normal", (char *)NULL);
    abort();
  }
#ifdef __linux__
  if (argc > 1 && strcmp(argv[1], "semantics") == 0) {
    void *zero = pvalloc(0);
    printf("pvalloc-zero-page=%d\n",
           zero && malloc_usable_size(zero) >= (size_t)sysconf(_SC_PAGESIZE));
    free(zero);
    errno = 0;
    void *invalid = aligned_alloc(3, 100);
    printf("aligned-invalid=%d errno=%d\n", invalid != NULL, errno);
    free(invalid);
  }
#endif
  if (argc > 1 && strcmp(argv[1], "attach") == 0) {
    puts("ready");
    fflush(stdout);
    sleep(15);
  }
  if (argc > 1 && strcmp(argv[1], "wait") == 0)
    sleep(3);
  if (argc > 1 && strcmp(argv[1], "immediate") == 0)
    _exit(0);
  return retained == NULL;
}
