/* FERB's background agent (L27): the executable of FERB.app/Contents/Helpers/FERB.app, which launchd starts.
 *
 * It embeds the app's own Python (it links the app's libpython; PyConfig.home is the app's Python) and, because it is
 * an app's executable, macOS names FERB, not python3.12, in permission prompts, Privacy & Security and Login Items;
 * the agent app's Info.plist carries the usage descriptions. This binary is what holds FERB's macOS permissions, so
 * code here enforces where it may be used (rule 5), before Python starts:
 *  - Only FERB's own entry points run, exactly as FERB starts them: `-m ferb.daemon` (the LaunchAgent),
 *    `-I -m ferb.cli.mcp` (the runtimes' MCP shim) and `-I -m ferb.memory.card_reader` (episode-card reads, Task 22:
 *    its input comes on stdin and its database from the owner's FERB home, so it takes no path); the daemon starts the
 *    last two through sys.executable. `-c`, scripts, other modules, extra arguments and any other option are refused,
 *    however the agent was started (launchd, `open`, a shell).
 *  - Only the owner's FERB home: FERB_HOME unset is the owner's ~/.ferb (from the account database, never $HOME); set,
 *    it must be that same folder (device and inode), and Python is then given the owner's path, not the one passed.
 *  - A clean environment, built here: HOME, USER, LOGNAME and SHELL from the account database, a fixed PATH, the
 *    owner's own temporary folder, FERB_HOME, and only the variables FERB needs carried over (locale, time zone,
 *    launchd's XPC_*, FERB_APP_BUILD, the shim's FERB_RUN_TOKEN/FERB_RUN_TOKEN_FILE). Nothing else reaches Python or
 *    the processes FERB starts: no PYTHON* variable (CPython reads some even in isolated mode; PYTHONEXECUTABLE sets
 *    sys.executable on macOS), no __PYVENV_LAUNCHER__, no proxy or certificate override.
 *  - sys.executable is this executable's real path (not argv[0]); Python runs isolated (no user site-packages, no
 *    working folder on sys.path) and never writes bytecode into the signed bundle: CPython's "customized Python always
 *    running in isolated mode" (docs.python.org/3.12/c-api/init_config.html). */
#include <Python.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

extern char **environ;

static int is_arg(int argc, char **argv, int i, const char *value) {
    return i < argc && strcmp(argv[i], value) == 0;
}

/* FERB's own entry points, exactly as FERB starts them; nothing else. */
static int entry_point(int argc, char **argv) {
    if (argc == 3 && is_arg(argc, argv, 1, "-m") && is_arg(argc, argv, 2, "ferb.daemon")) return 1;
    if (argc == 4 && is_arg(argc, argv, 1, "-I") && is_arg(argc, argv, 2, "-m")
            && (is_arg(argc, argv, 3, "ferb.cli.mcp") || is_arg(argc, argv, 3, "ferb.memory.card_reader")))
        return 1;
    return 0;
}

/* FERB_HOME as given is the owner's ~/.ferb (or unset): 0 when it names another folder. */
static int owners_home(const char *owner) {
    const char *given = getenv("FERB_HOME");
    if (given == NULL || given[0] == '\0') return 1;
    struct stat a, b;
    return stat(given, &a) == 0 && stat(owner, &b) == 0 && a.st_dev == b.st_dev && a.st_ino == b.st_ino;
}

/* The variables carried over from the environment the agent was started with, by exact name ("LC_" by prefix). */
static int carried(const char *entry) {
    static const char *names[] = {"LANG", "TZ", "__CF_USER_TEXT_ENCODING", "XPC_SERVICE_NAME", "XPC_FLAGS",
                                  "FERB_APP_BUILD", "FERB_RUN_TOKEN", "FERB_RUN_TOKEN_FILE", NULL};
    const char *eq = strchr(entry, '=');
    if (eq == NULL) return 0;                 /* an entry without a value is dropped */
    size_t len = (size_t)(eq - entry);
    if (len > 3 && strncmp(entry, "LC_", 3) == 0) return 1;
    for (int i = 0; names[i] != NULL; i++)
        if (strlen(names[i]) == len && strncmp(entry, names[i], len) == 0) return 1;
    return 0;
}

static char *pair(const char *name, const char *value) {
    size_t size = strlen(name) + strlen(value) + 2;
    char *out = malloc(size);
    if (out != NULL) snprintf(out, size, "%s=%s", name, value);
    return out;
}

/* The agent's whole environment, rebuilt (never edited in place). 0 when memory runs out. */
static int clean_environment(const struct passwd *pw, const char *ferb_home) {
    size_t count = 0;
    for (char **e = environ; *e != NULL; e++) count++;
    char **fresh = calloc(count + 8, sizeof(char *));
    if (fresh == NULL) return 0;
    size_t n = 0;
    char tmp[PATH_MAX] = "";
    fresh[n++] = pair("HOME", pw->pw_dir);
    fresh[n++] = pair("USER", pw->pw_name);
    fresh[n++] = pair("LOGNAME", pw->pw_name);
    fresh[n++] = pair("SHELL", pw->pw_shell && pw->pw_shell[0] ? pw->pw_shell : "/bin/zsh");
    fresh[n++] = pair("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    fresh[n++] = pair("FERB_HOME", ferb_home);
    if (confstr(_CS_DARWIN_USER_TEMP_DIR, tmp, sizeof tmp) > 0 && tmp[0] == '/') fresh[n++] = pair("TMPDIR", tmp);
    for (size_t i = 0; i < n; i++)
        if (fresh[i] == NULL) return 0;
    for (char **e = environ; *e != NULL; e++)
        if (carried(*e)) fresh[n++] = *e;
    fresh[n] = NULL;
    environ = fresh;
    return 1;
}

int main(int argc, char **argv) {
    char exe[PATH_MAX], real[PATH_MAX], folder[PATH_MAX], home[PATH_MAX + 32], python[PATH_MAX], owner[PATH_MAX];
    uint32_t size = sizeof exe;
    if (!entry_point(argc, argv)) {           /* opened from Finder, or anything that isn't FERB starting itself */
        fprintf(stderr, "FERB's background agent runs only FERB's own entry points; open FERB.app instead.\n");
        return 64;
    }
    struct passwd *pw = getpwuid(getuid());
    if (pw == NULL || pw->pw_dir == NULL || pw->pw_name == NULL) {
        fprintf(stderr, "FERB: can't read your account's home folder\n");
        return 70;
    }
    snprintf(owner, sizeof owner, "%s/.ferb", pw->pw_dir);
    if (!owners_home(owner)) {
        fprintf(stderr, "FERB's background agent runs only with your own FERB home: FERB_HOME isn't your ~/.ferb.\n");
        return 64;
    }
    if (!clean_environment(pw, owner)) {
        fprintf(stderr, "FERB: out of memory\n");
        return 70;
    }
    if (_NSGetExecutablePath(exe, &size) != 0 || !realpath(exe, real)) {
        fprintf(stderr, "FERB: can't find its own executable\n");
        return 70;
    }
    snprintf(folder, sizeof folder, "%s", real);
    *strrchr(folder, '/') = '\0';             /* <FERB.app>/Contents/Helpers/FERB.app/Contents/MacOS */
    snprintf(home, sizeof home, "%s/../../../../Resources/python", folder);
    if (!realpath(home, python)) {
        fprintf(stderr, "FERB.app is incomplete (no %s); download FERB again\n", home);
        return 70;
    }
    PyStatus status;
    PyConfig config;
    PyConfig_InitPythonConfig(&config);
    config.isolated = 1;
    config.write_bytecode = 0;
    status = PyConfig_SetBytesString(&config, &config.home, python);
    if (PyStatus_Exception(status)) goto fail;
    status = PyConfig_SetBytesString(&config, &config.executable, real);    /* sys.executable: never argv[0] */
    if (PyStatus_Exception(status)) goto fail;
    status = PyConfig_SetBytesArgv(&config, argc, argv);
    if (PyStatus_Exception(status)) goto fail;
    status = Py_InitializeFromConfig(&config);
    if (PyStatus_Exception(status)) goto fail;
    PyConfig_Clear(&config);
    return Py_RunMain();
fail:
    PyConfig_Clear(&config);
    if (PyStatus_IsExit(status)) return status.exitcode;
    Py_ExitStatusException(status);
}
