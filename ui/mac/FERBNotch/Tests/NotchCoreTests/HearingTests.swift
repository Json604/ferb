import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB.app's hearing modes (S/Hearing.swift): the home rule first, the daemon's lease, and a normal start otherwise.
final class HearingTests: XCTestCase {
    private var root = ""
    private var home = ""
    private var audio = ""

    override func setUpWithError() throws {
        root = (FileManager.default.temporaryDirectory.appendingPathComponent("Hearing-\(UUID().uuidString)").path)
        home = root + "/.ferb"
        audio = home + "/workspace/audio"
        try FileManager.default.createDirectory(atPath: audio, withIntermediateDirectories: true)
        let dir = root
        addTeardownBlock { try? FileManager.default.removeItem(atPath: dir) }
    }

    override func tearDown() { Hearing.onLeaseLost = { exit(0) } }

    private func lease(_ name: String = ".status-1.json", in folder: String? = nil) -> String {
        let path = (folder ?? audio) + "/" + name
        FileManager.default.createFile(atPath: path, contents: Data(), attributes: [.posixPermissions: 0o600])
        return path
    }

    func testAnUnknownOrMissingFirstArgumentStartsTheNormalApp() {
        XCTAssertNil(Hearing.run(["FERBNotch"], givenHome: nil, account: home))
        XCTAssertNil(Hearing.run(["FERBNotch", "--service"], givenHome: nil, account: home))
        XCTAssertNil(Hearing.run(["FERBNotch", "-NSDocumentRevisionsDebugMode", "YES"], givenHome: nil, account: home))
    }

    func testAForeignFerbHomeIsRefusedBeforeAnythingIsAnswered() throws {
        let foreign = root + "/x"
        try FileManager.default.createDirectory(atPath: foreign + "/workspace/audio", withIntermediateDirectories: true)
        let answer = lease(in: foreign + "/workspace/audio")
        for argv in [["status", answer], ["transcribe", answer, answer], ["request-speech"]] {
            XCTAssertEqual(Hearing.run(["FERBNotch"] + argv, givenHome: foreign, account: home), 64, "\(argv)")
            XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: answer)), Data())
        }
        XCTAssertEqual(Hearing.run(["FERBNotch", "status", answer], givenHome: nil, account: nil), 64)   // no account
    }

    func testAModeWithoutItsArgumentsDoesNothing() {
        XCTAssertEqual(Hearing.run(["FERBNotch", "status"], givenHome: nil, account: home), 64)
        XCTAssertEqual(Hearing.run(["FERBNotch", "transcribe", "a.wav"], givenHome: nil, account: home), 64)
    }

    func testOnlyTheDaemonsEmptyAnswerFileInTheAudioFolderIsAccepted() throws {
        let ok = lease()
        let fd = try XCTUnwrap(Hearing.answerFile(ok, home: home))
        Hearing.writeAnswer(fd, ["speech": "granted"])
        XCTAssertTrue(try String(contentsOfFile: ok, encoding: .utf8).contains("granted"))
        XCTAssertNil(Hearing.answerFile(ok, home: home))                       // an answer is never replaced
        XCTAssertNil(Hearing.answerFile(audio + "/missing.json", home: home))   // never creates a file
        XCTAssertNil(Hearing.answerFile(lease("x.json", in: root), home: home)) // not in FERB's audio folder
        let source = lease(".src.json")
        let second = audio + "/.second.json"
        XCTAssertEqual(link(source, second), 0)
        XCTAssertNil(Hearing.answerFile(second, home: home))                    // a second name for it
        let target = lease(".target.json")
        let symlink = audio + "/.symlink.json"
        try FileManager.default.createSymbolicLink(atPath: symlink, withDestinationPath: target)
        XCTAssertNil(Hearing.answerFile(symlink, home: home))                   // never through a link
    }

    func testDeletingTheAnswerFileStopsHearingAndNothingIsLeftBehind() throws {
        let stopped = expectation(description: "stopped")
        Hearing.onLeaseLost = { stopped.fulfill() }
        let path = lease()
        let fd = try XCTUnwrap(Hearing.answerFile(path, home: home))
        try FileManager.default.removeItem(atPath: path)
        wait(for: [stopped], timeout: 5)
        Hearing.writeAnswer(fd, ["text": "secret words"])                       // lands in the unlinked file
        XCTAssertFalse(FileManager.default.fileExists(atPath: path))
    }

    func testASoundOutsideTheAudioFolderIsRefusedBeforeSpeechIsTouched() throws {
        let path = lease()
        let stray = root + "/stray.wav"
        FileManager.default.createFile(atPath: stray, contents: Data([0]))
        XCTAssertEqual(Hearing.run(["FERBNotch", "transcribe", stray, path], givenHome: nil, account: home), 0)
        let text = try String(contentsOfFile: path, encoding: .utf8)
        XCTAssertTrue(text.contains("audio folder") && text.contains("failed"))
    }
}
