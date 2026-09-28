/* Spike 15: stable launcher. launchd runs this; it starts argv[1..] as a CHILD process (posix_spawn) and waits,
   so the launcher stays the process TCC attributes the child's requests to. */
#include <spawn.h>
#include <stdio.h>
#include <sys/wait.h>
extern char **environ;
int main(int argc, char *argv[]) {
    if (argc < 2) { fprintf(stderr, "usage: launcher <program> [args...]\n"); return 64; }
    pid_t pid;
    int rc = posix_spawn(&pid, argv[1], NULL, NULL, &argv[1], environ);
    if (rc != 0) { fprintf(stderr, "spawn failed: %d\n", rc); return 70; }
    int status;
    if (waitpid(pid, &status, 0) < 0) return 71;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 72;
}
