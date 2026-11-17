import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// "Load more" (kb PHN-609; Android's FilesMoreTest): a page at a time, added in place, while the Mac's page carries a
/// marker; with none, nothing more is offered.
@MainActor
final class FilesPagingTests: XCTestCase {
    private var rig: Rig!
    override func setUp() async throws { rig = Rig(); rig.session.unlocked(); rig.session.foreground(true) }
    override func tearDown() async throws { rig.tearDown() }

    private func entry(_ n: Int) -> FileEntry {
        FileEntry(name: "f\(n).txt", path: "~/Big/f\(n).txt", dir: false, link: false, size: 1, modified: 0, locked: false)
    }
    private func hit(_ n: Int) -> FoundFile {
        FoundFile(name: "tax\(n).pdf", path: "~/Big/tax\(n).pdf", dir: false, size: 1, modified: 0, source: "", at: nil, taskId: nil, taskName: nil)
    }

    // decision: PHN-609, PHN-606
    func testLoadMoreAddsTheNextPageOfAFolderAndOfASearchInPlace() async {
        let phone = rig.phone
        let first = Folder(path: "~/Big", parent: "~", locked: false, entries: (1...3).map(entry), total: 7, cut: true, from: 0, next: "p2")
        let second = Folder(path: "~/Big", parent: "~", locked: false, entries: (4...6).map(entry), total: 7, cut: true, from: 3, next: "p3")
        let last = Folder(path: "~/Big", parent: "~", locked: false, entries: [entry(7)], total: 7, cut: false, from: 6, next: nil)
        phone.folders[first.path] = first
        phone.morePages = [second, last]
        let vm = rig.model()
        await vm.openFolder("~/Big").value
        XCTAssertEqual(vm.ui.files.folder?.next, "p2")
        await vm.folderMore()?.value
        XCTAssertEqual(vm.ui.files.folder?.entries.count, 6)
        XCTAssertTrue(phone.calls.contains("files-more:~/Big:p2:false"))
        await vm.folderMore()?.value
        XCTAssertEqual(vm.ui.files.folder?.entries.map(\.name), (1...7).map { "f\($0).txt" })
        XCTAssertNil(vm.ui.files.folder?.next, "no more after the last page")
        XCTAssertNil(vm.folderMore())
        // a search: the next page of matches under the first; the 100,000-name bound still says so on the last page
        let p1 = FoundFiles(path: "~/Big", query: "tax", files: [hit(1), hit(2)], looked: 50_000, skipped: 0, cut: true, incomplete: false, from: 0, next: "s2")
        let p2 = FoundFiles(path: "~/Big", query: "tax", files: [hit(3)], looked: 100_000, skipped: 0, cut: false, incomplete: true, from: 2, next: nil)
        phone.found = p1
        phone.moreFound = [p2]
        await vm.search("tax")?.value
        XCTAssertEqual(vm.ui.files.search?.found?.next, "s2")
        await vm.searchMore()?.value
        XCTAssertEqual(vm.ui.files.search?.found?.files.count, 3)
        XCTAssertNil(vm.ui.files.search?.found?.next)
        XCTAssertEqual(vm.ui.files.search?.found?.incomplete, true)
        XCTAssertTrue(phone.calls.contains("search-more:tax:s2"))
    }
}
