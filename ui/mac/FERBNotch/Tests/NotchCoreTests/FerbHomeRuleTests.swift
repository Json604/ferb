import XCTest
@testable import NotchCore

final class FerbHomeRuleTests: XCTestCase {
    private func folders() throws -> (account: String, link: String, other: String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("FerbHomeRule-\(UUID().uuidString)")
        let fm = FileManager.default
        let account = root.appendingPathComponent("Users/owner/.ferb")
        let other = root.appendingPathComponent("x")
        try fm.createDirectory(at: account, withIntermediateDirectories: true)
        try fm.createDirectory(at: other, withIntermediateDirectories: true)
        let link = root.appendingPathComponent("link-to-ferb")
        try fm.createSymbolicLink(at: link, withDestinationURL: account)
        addTeardownBlock { try? fm.removeItem(at: root) }
        return (account.path, link.path, other.path)
    }

    func testUnsetEmptyOrTheAccountsOwnFolderIsTheHome() throws {
        let f = try folders()
        XCTAssertEqual(resolveFerbHome(nil, account: f.account), .home(f.account))
        XCTAssertEqual(resolveFerbHome("", account: f.account), .home(f.account))
        XCTAssertEqual(resolveFerbHome(f.account, account: f.account), .home(f.account))
        XCTAssertEqual(resolveFerbHome(f.link, account: f.account), .home(f.account))   // same device and inode
    }

    func testAnotherFolderOrNoAccountIsRefused() throws {
        let f = try folders()
        for foreign in [f.other, f.other + "/missing"] {
            guard case .refused(let why) = resolveFerbHome(foreign, account: f.account) else {
                return XCTFail("\(foreign) was accepted") }
            XCTAssertTrue(why.contains("FERB_HOME"))
        }
        guard case .refused = resolveFerbHome(f.account, account: f.account + "-gone") else { return XCTFail() }
        guard case .refused = resolveFerbHome(nil, account: nil) else { return XCTFail() }
    }
}
