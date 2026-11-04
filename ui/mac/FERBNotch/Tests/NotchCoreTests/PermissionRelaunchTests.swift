import XCTest
@testable import NotchCore

/// APP-661: macOS's Quit & Reopen relaunches FERB from System Settings about 3 s after FERB exits; closing System
/// Settings sooner cancelled it on the owner's Mac (2026-11-04), so a quit System Settings sent leaves a watcher.
final class PermissionRelaunchTests: XCTestCase {
    // decision: APP-661
    func testOnlyAQuitSystemSettingsSentWithoutALogoutReasonReopens() {
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.settings.PrivacySecurity.extension",
                                                      hasQuitReason: false))
        XCTAssertTrue(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.systempreferences", hasQuitReason: false))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.systempreferences", hasQuitReason: true))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: "com.apple.ActivityMonitor", hasQuitReason: false))
        XCTAssertFalse(PermissionRelaunch.reopensAfter(senderBundleID: nil, hasQuitReason: false))
    }

    /// With FERB gone, the sender gone and FERB not running again, the watcher opens FERB.app (here: an opener that
    /// touches a file in the test's own folder, so nothing is launched).
    // decision: APP-661
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
