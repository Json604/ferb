import XCTest
@testable import NotchCore

/// APP-684: macOS's Quit & Reopen relaunches FERB from System Settings about 3 s after FERB exits; closing System
/// Settings sooner cancelled it on the owner's Mac (2026-11-04), so a quit System Settings sent leaves a watcher.
final class PermissionRelaunchTests: XCTestCase {
    /// Live 2026-11-04 09:32:17: Full Disk Access's Quit & Reopen came from Privacy & Security with a quit reason, the
    /// watcher never started and FERB stayed gone. Any quit System Settings sends reopens, except a log out, restart
    /// or shut down.
    // decision: APP-684
    func testAQuitSystemSettingsSentReopensUnlessTheSessionEnds() {
        let extensionID = "com.apple.settings.PrivacySecurity.extension"
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: extensionID, quitReason: nil))
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: extensionID,
                                                      quitReason: PermissionRelaunch.code("quia")))
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.systempreferences", quitReason: nil))
        for ends in ["logo", "rlgo", "rrst", "rsdn", "rest", "shut"] {
            XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.systempreferences",
                                                           quitReason: PermissionRelaunch.code(ends)))
        }
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.ActivityMonitor", quitReason: nil))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: nil, quitReason: nil))
        XCTAssertEqual(PermissionRelaunch.text(PermissionRelaunch.code("rlgo")), "rlgo")
    }

    /// The sender (System Settings' extension) can outlive the quit by minutes while Settings stays open: after a few
    /// looks with FERB still not back, the watcher opens it anyway.
    // decision: APP-684
    func testTheWatcherOpensFERBAfterAFewLooksWhileTheSenderStillRuns() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("relaunch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let opened = folder.appendingPathComponent("FERB.app.opened")
        let argv = PermissionRelaunch.arguments(ferbPID: try finishedPID(), senderPID: getpid(),   // the sender lives
                                                executable: folder.appendingPathComponent("NoSuchFERB").path,
                                                app: opened.path, poll: 0.01, reads: 2, opener: "/usr/bin/touch")
        let watcher = Process()
        watcher.executableURL = URL(fileURLWithPath: argv[0])
        watcher.arguments = Array(argv.dropFirst())
        try watcher.run()
        watcher.waitUntilExit()
        XCTAssertEqual(watcher.terminationStatus, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: opened.path))
    }

    /// With FERB gone, the sender gone and FERB not running again, the watcher opens FERB.app (here: an opener that
    /// touches a file in the test's own folder, so nothing is launched).
    // decision: APP-684
    func testTheWatcherOpensFERBWhenTheSenderLeftWithoutRelaunchingIt() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("relaunch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let opened = folder.appendingPathComponent("FERB.app.opened")
        let gone = try finishedPID()
        let watcher = Process()
        let argv = PermissionRelaunch.arguments(ferbPID: gone, senderPID: gone,
                                                executable: folder.appendingPathComponent("NoSuchFERB").path,
                                                app: opened.path, poll: 0.01, opener: "/usr/bin/touch")
        watcher.executableURL = URL(fileURLWithPath: argv[0])
        watcher.arguments = Array(argv.dropFirst())
        try watcher.run()
        watcher.waitUntilExit()
        XCTAssertEqual(watcher.terminationStatus, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: opened.path))
    }

    /// A process id that has already exited.
    private func finishedPID() throws -> pid_t {
        let done = Process()
        done.executableURL = URL(fileURLWithPath: "/usr/bin/true")
        try done.run()
        done.waitUntilExit()
        return done.processIdentifier
    }
}
