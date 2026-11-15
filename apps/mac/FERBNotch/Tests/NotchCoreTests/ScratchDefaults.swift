import Foundation
import XCTest

extension XCTestCase {
    /// A preferences suite of this test's own, removed when the test ends. Its file lives in a folder under the temporary
    /// directory (a suite named by an absolute path is stored at that path), never in `~/Library/Preferences`, so a
    /// test run leaves nothing behind there even when a model writes after its test ended.
    func scratchDefaults(_ prefix: String = "FERBNotchTests") -> UserDefaults {
        let suite = Self.scratchSuiteName(prefix)
        addTeardownBlock { Self.removeScratchSuite(suite) }
        return UserDefaults(suiteName: suite)!
    }

    static func scratchSuiteName(_ prefix: String = "FERBNotchTests") -> String {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FERBNotchTests", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder.appendingPathComponent(prefix + "-" + UUID().uuidString).path
    }

    /// A test's own suite gone: its values, then its file.
    static func removeScratchSuite(_ suite: String) {
        UserDefaults().removePersistentDomain(forName: suite)
        try? FileManager.default.removeItem(atPath: suite + ".plist")
    }
}
