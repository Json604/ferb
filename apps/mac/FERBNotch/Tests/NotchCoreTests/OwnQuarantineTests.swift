import Darwin
import XCTest
@testable import NotchCore

final class OwnQuarantineTests: XCTestCase {
    private let mark = "0083;6727e3a1;Safari;"

    private func quarantine(_ url: URL, follow: Bool = false) {
        let flags: Int32 = follow ? 0 : XATTR_NOFOLLOW
        let ok = url.withUnsafeFileSystemRepresentation { path in
            setxattr(path!, OwnQuarantine.attribute, mark, mark.utf8.count, 0, flags) == 0
        }
        XCTAssertTrue(ok, "couldn't mark \(url.path)")
    }

    /// A copy of FERB.app as Finder leaves it after a drag from a downloaded disk image: every item quarantined, plus a
    /// link inside pointing at a quarantined file outside the bundle.
    private func bundle() throws -> (app: URL, nested: [URL], outside: URL, link: URL) {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("OwnQuarantine-\(UUID().uuidString)")
        addTeardownBlock { try? fm.removeItem(at: root) }
        let app = root.appendingPathComponent("FERB.app")
        let helpers = app.appendingPathComponent("Contents/Helpers")
        let python = app.appendingPathComponent("Contents/Resources/python/bin")
        try fm.createDirectory(at: helpers, withIntermediateDirectories: true)
        try fm.createDirectory(at: python, withIntermediateDirectories: true)
        let secrets = helpers.appendingPathComponent("ferb-secrets")
        let interpreter = python.appendingPathComponent("python3.12")
        try Data("x".utf8).write(to: secrets)
        try Data("x".utf8).write(to: interpreter)
        let outside = root.appendingPathComponent("someone-elses.txt")
        try Data("x".utf8).write(to: outside)
        let link = helpers.appendingPathComponent("elsewhere")
        try fm.createSymbolicLink(at: link, withDestinationURL: outside)
        let nested = [app.appendingPathComponent("Contents"), helpers, python, secrets, interpreter]
        for url in nested + [app, outside] { quarantine(url) }
        quarantine(link)
        return (app, nested, outside, link)
    }

    // decision: APP-601
    func testFerbClearsQuarantineFromItsOwnBundleOnlyNeverThroughALink() throws {
        let b = try bundle()
        XCTAssertGreaterThanOrEqual(OwnQuarantine.clear(bundle: b.app), 7)
        for url in b.nested + [b.app, b.link] { XCTAssertFalse(OwnQuarantine.has(url), url.path) }
        XCTAssertTrue(OwnQuarantine.has(b.outside))                   // the link's target outside is left alone
        XCTAssertEqual(OwnQuarantine.clear(bundle: b.app), 0)         // nothing left: the next launch does nothing
    }

    // decision: APP-601
    func testATranslocatedCopyOrSomethingThatIsntAnAppIsLeftAlone() throws {
        let b = try bundle()
        let folder = b.app.deletingLastPathComponent()
        XCTAssertEqual(OwnQuarantine.clear(bundle: folder), 0)        // not an app bundle
        let fm = FileManager.default
        let translocated = folder.appendingPathComponent("AppTranslocation/X/d/FERB.app")
        try fm.createDirectory(at: translocated.deletingLastPathComponent(), withIntermediateDirectories: true)
        try fm.copyItem(at: b.app, to: translocated)
        quarantine(translocated)
        XCTAssertEqual(OwnQuarantine.clear(bundle: translocated), 0)
        XCTAssertTrue(OwnQuarantine.has(translocated))
        XCTAssertTrue(OwnQuarantine.has(b.app))
    }
}
