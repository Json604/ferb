import AppKit
import Darwin
import NotchCore
import os

/// APP-689: a quit System Settings sent (macOS's Quit & Reopen after a privacy switch) leaves a detached watcher that
/// reopens FERB.app if macOS's own relaunch doesn't come (PermissionRelaunch). Read while the quit is being handled;
/// started as FERB terminates.
@MainActor
enum PermissionRelaunchWatcher {
    private static let log = Logger(subsystem: "com.ferb.FERB", category: "relaunch")
    /// The sender of the quit being handled, when it is one to reopen after.
    private static var sender: pid_t?

    /// Called from applicationShouldTerminate: notes the quit Apple Event's sender when System Settings sent it.
    static func noteQuit() {
        guard let event = NSAppleEventManager.shared().currentAppleEvent,
              event.eventClass == AEEventClass(kCoreEventClass), event.eventID == AEEventID(kAEQuitApplication),
              let pid = event.attributeDescriptor(forKeyword: AEKeyword(keySenderPIDAttr))?.int32Value, pid > 1
        else { return }
        let bundleID = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier
        let given = event.attributeDescriptor(forKeyword: AEKeyword(kAEQuitReason))
            ?? event.paramDescriptor(forKeyword: AEKeyword(kAEQuitReason))
        let reason = given.map { $0.enumCodeValue != 0 ? $0.enumCodeValue : $0.typeCodeValue }
        let reopens = PermissionRelaunch.reopensAfter(senderBundleID: bundleID)   // the reason is only logged (APP-689)
        let said = reason.map { "reason '\(PermissionRelaunch.text($0))'" } ?? "no reason"
        log.notice("Quit asked by \(bundleID ?? "an unknown process", privacy: .public) (pid \(pid), \(said, privacy: .public)): \(reopens ? "a reopen watcher starts as FERB exits" : "no reopen watcher", privacy: .public)")
        if reopens { sender = pid }
    }

    /// Called from applicationWillTerminate: starts the watcher (its own session, no inherited descriptors, so it
    /// outlives FERB and holds nothing of it open). Only a packaged FERB.app.
    /// `poll` is the grant poll (APP-510's ~1.5 s), read only when a watcher starts.
    static func startIfNoted(poll: @autoclosure () -> Duration, reads: @autoclosure () -> Int) {
        guard let sender, AppBundle.isApp, let executable = Bundle.main.executablePath else { return }
        let every = poll().components
        let seconds = Double(every.seconds) + Double(every.attoseconds) / 1e18
        let argv = PermissionRelaunch.arguments(ferbPID: getpid(), senderPID: sender, executable: executable,
                                                app: Bundle.main.bundleURL.path, poll: seconds, reads: reads())
        var attributes: posix_spawnattr_t?
        var actions: posix_spawn_file_actions_t?
        posix_spawnattr_init(&attributes)
        posix_spawn_file_actions_init(&actions)
        defer {
            posix_spawnattr_destroy(&attributes)
            posix_spawn_file_actions_destroy(&actions)
        }
        let setsid: Int32 = 0x0400               // POSIX_SPAWN_SETSID: its own session, outside FERB's process group
        let closeOthers: Int32 = 0x4000          // POSIX_SPAWN_CLOEXEC_DEFAULT: none of FERB's descriptors carried
        posix_spawnattr_setflags(&attributes, Int16(setsid | closeOthers))
        for fd: Int32 in 0...2 {
            posix_spawn_file_actions_addopen(&actions, fd, "/dev/null", fd == 0 ? O_RDONLY : O_WRONLY, 0)
        }
        var cArgs = argv.map { strdup($0) } + [nil]
        defer { cArgs.forEach { free($0) } }
        var child: pid_t = 0
        let status = posix_spawn(&child, "/bin/sh", &actions, &attributes, &cArgs, environ)
        // Its own steps go to the system log tagged ferb-relaunch (PermissionRelaunch.script).
        log.notice("Reopen watcher \(status == 0 ? "started (pid \(child))" : "failed (\(status))", privacy: .public) for \(Bundle.main.bundleURL.path, privacy: .public) (sender pid \(sender))")
    }
}
