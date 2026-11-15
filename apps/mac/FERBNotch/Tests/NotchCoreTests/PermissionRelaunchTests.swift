import XCTest
@testable import NotchCore

/// APP-689: macOS's Quit & Reopen relaunches FERB from System Settings about 3 s after FERB exits, or not at all
/// (macOS 27.2), so a quit System Settings sent leaves a watcher that opens FERB.app.
final class PermissionRelaunchTests: XCTestCase {
    /// Live 2026-11-04 11:16:02 and 11:17:54: Screen Recording's Quit & Reopen came from Privacy & Security with
    /// kAEQuitReason "rlgo" (kAEReallyLogOut); APP-684 took it for a log out, started no watcher and FERB stayed gone.
    /// Any quit System Settings sends reopens, whatever its reason; other senders never do.
    // decision: APP-689
    func testAQuitSystemSettingsSentReopensWhateverItsReason() {
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.settings.PrivacySecurity.extension"))
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.systempreferences"))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.loginwindow"))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.ActivityMonitor"))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: nil))
        XCTAssertEqual(PermissionRelaunch.text(PermissionRelaunch.code("rlgo")), "rlgo")
    }

    /// The sender (System Settings' extension) can outlive the quit by minutes while Settings stays open: after a few
    /// looks with FERB still not back, the watcher opens it anyway, and reports each step.
    // decision: APP-689
    func testTheWatcherOpensFERBAfterAFewLooksWhileTheSenderStillRuns() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("relaunch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let opened = folder.appendingPathComponent("FERB.app.opened")
        let argv = PermissionRelaunch.arguments(ferbPID: try finishedPID(), senderPID: getpid(),   // the sender lives
                                                executable: folder.appendingPathComponent("NoSuchFERB").path,
                                                app: opened.path, poll: 0.01, reads: 2, opener: "/usr/bin/touch",
                                                logger: "echo")
        let watcher = Process()
        let said = Pipe()
        watcher.executableURL = URL(fileURLWithPath: argv[0])
        watcher.arguments = Array(argv.dropFirst())
        watcher.standardOutput = said
        try watcher.run()
        watcher.waitUntilExit()
        XCTAssertEqual(watcher.terminationStatus, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: opened.path))
        let lines = String(decoding: said.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        XCTAssertTrue(lines.contains("exited"))
        XCTAssertTrue(lines.contains("opening \(opened.path)"))
    }

    /// With FERB gone, the sender gone and FERB not running again, the watcher opens FERB.app (here: an opener that
    /// touches a file in the test's own folder, so nothing is launched).
    // decision: APP-689
    func testTheWatcherOpensFERBWhenTheSenderLeftWithoutRelaunchingIt() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("relaunch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let opened = folder.appendingPathComponent("FERB.app.opened")
        let gone = try finishedPID()
        let watcher = Process()
        let argv = PermissionRelaunch.arguments(ferbPID: gone, senderPID: gone,
                                                executable: folder.appendingPathComponent("NoSuchFERB").path,
                                                app: opened.path, poll: 0.01, opener: "/usr/bin/touch", logger: ":")
        watcher.executableURL = URL(fileURLWithPath: argv[0])
        watcher.arguments = Array(argv.dropFirst())
        try watcher.run()
        watcher.waitUntilExit()
        XCTAssertEqual(watcher.terminationStatus, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: opened.path))
    }

    /// FERB already back (macOS relaunched it): the watcher opens nothing. APP-684's script read `$3` inside a shell
    /// function, where it is the function's own (empty) argument, so it never saw FERB running.
    // decision: APP-689
    func testTheWatcherOpensNothingWhenFERBIsRunningAgain() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("relaunch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let opened = folder.appendingPathComponent("FERB.app.opened")
        let ferb = Process()                                        // stands in for the relaunched FERB
        ferb.executableURL = URL(fileURLWithPath: "/bin/sleep")
        ferb.arguments = ["30"]
        try ferb.run()
        defer { ferb.terminate() }
        let argv = PermissionRelaunch.arguments(ferbPID: try finishedPID(), senderPID: getpid(),
                                                executable: "/bin/sleep", app: opened.path, poll: 0.01,
                                                opener: "/usr/bin/touch", logger: ":")
        let watcher = Process()
        watcher.executableURL = URL(fileURLWithPath: argv[0])
        watcher.arguments = Array(argv.dropFirst())
        try watcher.run()
        watcher.waitUntilExit()
        XCTAssertEqual(watcher.terminationStatus, 0)
        XCTAssertFalse(FileManager.default.fileExists(atPath: opened.path))
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
