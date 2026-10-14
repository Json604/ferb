/* FERB's background agent (L27): the executable of FERB.app/Contents/Helpers/FERB.app, which launchd starts.
 *
 * It embeds the app's own Python (it links the app's libpython; PyConfig.home is the app's Python) and, because it is
 * an app's executable, macOS names FERB, not python3.12, in permission prompts, Privacy & Security and Login Items;
 * the agent app's Info.plist carries the usage descriptions. This binary is what holds FERB's macOS permissions, so
 * code here enforces where it may be used (rule 5), before Python starts:
 *  - Only FERB's own entry points run, exactly as FERB starts them: `-m ferb.daemon` (the LaunchAgent),
 *    `-I -m ferb.cli.mcp` (the runtimes' MCP shim, started by the daemon through sys.executable) and
 *    `-I -m ferb.device.detector <log>` (the background test's detector). `-c`, scripts, other modules and any other
 *    option are refused.
 *  - Only the owner's FERB home: FERB_HOME unset becomes the owner's ~/.ferb (from the account database, never $HOME);
 *    set, it must be that same folder (by device and inode).
 *  - Every PYTHON* variable and __PYVENV_LAUNCHER__ is removed from the environment: CPython reads some of them even in
 *    isolated mode (PYTHONEXECUTABLE sets sys.executable on macOS), and the processes FERB starts inherit it.
 *  - Python then runs isolated (no user site-packages, no working folder on sys.path) and never writes bytecode into
 *    the signed bundle: CPython's "customized Python always running in isolated mode"
 *    (docs.python.org/3.12/c-api/init_config.html). */
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
    if (argc == 4 && is_arg(argc, argv, 1, "-I") && is_arg(argc, argv, 2, "-m") && is_arg(argc, argv, 3, "ferb.cli.mcp"))
        return 1;
    if (argc == 5 && is_arg(argc, argv, 1, "-I") && is_arg(argc, argv, 2, "-m")
            && is_arg(argc, argv, 3, "ferb.device.detector") && argv[4][0] != '-')
        return 1;
    return 0;
}

/* FERB_HOME: unset becomes the owner's ~/.ferb; set, it must be that folder. 0 when refused. */
static int owners_home(void) {
    struct passwd *pw = getpwuid(getuid());
    char owner[PATH_MAX];
    if (pw == NULL || pw->pw_dir == NULL) return 0;
    snprintf(owner, sizeof owner, "%s/.ferb", pw->pw_dir);
    const char *given = getenv("FERB_HOME");
    if (given == NULL || given[0] == '\0') return setenv("FERB_HOME", owner, 1) == 0;
    struct stat a, b;
    return stat(given, &a) == 0 && stat(owner, &b) == 0 && a.st_dev == b.st_dev && a.st_ino == b.st_ino;
}

static void scrub_python_environment(void) {
    for (;;) {                                /* unsetenv changes environ: start over after each removal */
        char name[256] = "";
        for (char **e = environ; *e != NULL; e++) {
            const char *eq = strchr(*e, '=');
            size_t len = eq ? (size_t)(eq - *e) : strlen(*e);
            if (len < sizeof name && (strncmp(*e, "PYTHON", 6) == 0 || strncmp(*e, "__PYVENV_LAUNCHER__=", 20) == 0)) {
                memcpy(name, *e, len);
                name[len] = '\0';
                break;
            }
        }
        if (name[0] == '\0' || unsetenv(name) != 0) return;
    }
}

int main(int argc, char **argv) {
    char exe[PATH_MAX], real[PATH_MAX], home[PATH_MAX + 32], python[PATH_MAX];
    uint32_t size = sizeof exe;
    if (!entry_point(argc, argv)) {           /* opened from Finder, or anything that isn't FERB starting itself */
        fprintf(stderr, "FERB's background agent runs only FERB's own entry points; open FERB.app instead.\n");
        return 64;
    }
    if (!owners_home()) {
        fprintf(stderr, "FERB's background agent runs only with your own FERB home: FERB_HOME isn't your ~/.ferb.\n");
        return 64;
    }
    scrub_python_environment();
    if (_NSGetExecutablePath(exe, &size) != 0 || !realpath(exe, real)) {
        fprintf(stderr, "FERB: can't find its own executable\n");
        return 70;
    }
    *strrchr(real, '/') = '\0';               /* <FERB.app>/Contents/Helpers/FERB.app/Contents/MacOS */
    snprintf(home, sizeof home, "%s/../../../../Resources/python", real);
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
