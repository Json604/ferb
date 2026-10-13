/* FERB's background agent (L27): the executable of FERB.app/Contents/Helpers/FERB.app, which launchd starts.
 *
 * It is Python itself, embedded: it links the app's own libpython and runs it exactly like `python3.12` with the
 * arguments it was given (`-m ferb.daemon` from the LaunchAgent; `-m ferb.cli.mcp` or `-c …` when the daemon starts
 * its own helpers through sys.executable). Because it is an app's executable, macOS names FERB, not python3.12, in
 * permission prompts, Privacy & Security and Login Items, and the agent app's Info.plist carries the usage
 * descriptions. It always runs isolated (no PYTHON* variables, no user site-packages, no working folder on sys.path)
 * and never writes bytecode into the signed bundle. The pattern is CPython's "customized Python always running in
 * isolated mode" (docs.python.org/3.12/c-api/init_config.html), with PyConfig.home set to the app's Python. */
#include <Python.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    char exe[PATH_MAX], real[PATH_MAX], home[PATH_MAX + 32], python[PATH_MAX];
    uint32_t size = sizeof exe;
    if (argc < 2) {                       /* opened from Finder or `open`: launchd starts the service, not this */
        fprintf(stderr, "FERB's background service is started by launchd; open FERB.app instead.\n");
        return 64;
    }
    if (_NSGetExecutablePath(exe, &size) != 0 || !realpath(exe, real)) {
        fprintf(stderr, "FERB: can't find its own executable\n");
        return 70;
    }
    *strrchr(real, '/') = '\0';           /* <FERB.app>/Contents/Helpers/FERB.app/Contents/MacOS */
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
