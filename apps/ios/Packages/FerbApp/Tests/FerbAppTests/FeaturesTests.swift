import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import XCTest

/// T-1221's features over a fake phone (Android's FeaturesTest): the lock, Stop everything, the Mac's status, files,
/// Watch live, settings, tasks; and what iOS adds (the background checkpoint).
@MainActor
final class FeaturesTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }
    private var session: Session { rig.session }
    private var local: LocalFiles { rig.local }
    private var prompts: FakePrompts { rig.prompts }

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func exists(_ u: URL) -> Bool { FileManager.default.fileExists(atPath: u.path) }

    private func write(_ text: String, to u: URL) {
        try? FileManager.default.createDirectory(at: u.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? Data(text.utf8).write(to: u)
    }

    // decision: PHN-15, PHN-125, PHN-515, PHN-510
    func testLockingTellsTheMacOnceAndTheFetchedFilesGo() async {
        write("x", to: local.fetched.appendingPathComponent("k/a.txt"))
        let teller = LockTeller(session: session, api: phone, local: local)
        await settle()
        XCTAssertFalse(phone.calls.contains("lock"), "a cold start is locked already: nothing told")
        session.unlocked()
        await settle()
        session.lockNow()
        await eventually("the wipe") { !exists(local.fetched) }
        await eventually("told") { phone.calls.filter { $0 == "lock" }.count == 1 }
        withExtendedLifetime(teller) {}
    }

    // decision: PHN-13, PHN-125
    func testAnOfflineUnlockTellsTheLibraryAndReadsTheMacsNameWhenOnline() async {
        phone.link = FakePhone.status(.waiting, paired: false)
        let vm = rig.model()
        await vm.unlockApp().value
        // the prompt alone opened the app (the Mac out of reach): the library is told
        XCTAssertTrue(phone.calls.contains("unlocked"))
        XCTAssertFalse(session.locked)
        // online: the Mac's unlock, then its name
        session.lockNow()
        phone.link = FakePhone.status()
        phone.mac = MacInfo(name: "Studio Mac", pairedAt: "2026-11-10T09:00:00+05:30", phoneName: "iPhone", uploadFolder: nil)
        await vm.unlockApp().value
        await eventually("the Mac's name") { vm.ui.mac?.name == "Studio Mac" }
    }

    // decision: PHN-125, DMN-1
    func testStopEverythingAfterAPlainConfirmationWithNoPrompt() async {
        session.unlocked()
        phone.status = phoneStatus(stopped: true)
        let vm = rig.model()
        vm.show(.stopAll)
        await vm.stopEverything().value
        XCTAssertTrue(phone.calls.contains("stop-everything"))
        XCTAssertTrue(prompts.signers.isEmpty, "no prompt")
        XCTAssertNil(vm.sheet)
        XCTAssertEqual(vm.ui.note?.key, "stop_all_done")
        XCTAssertEqual(vm.ui.status?.stopped, true)
    }

    // decision: PHN-417, PHN-470, PHN-137, PHN-520
    func testAQuestionThatNeedsFaceIdAsksForItUpFrontAndAPlainNoNever() async {
        session.unlocked()
        phone.view = emptyView()
        phone.status = phoneStatus(asked("ap-1", needs(unlock: true, fingerprint: true)), asked("ap-2", needs(unlock: true)),
                                   asked("ap-3", needs(unlock: false)))
        let vm = rig.model()
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.questions.count, 3)
        await vm.answer("ap-1", .approve(granted: true, choice: 1)).value
        XCTAssertEqual(prompts.signers.map(\.kind), [.strong], "the Face ID key, before the first try")
        XCTAssertEqual(phone.calls.filter { $0.hasPrefix("answer") }, ["answer:ap-1:true"])
        await vm.answer("ap-2", .approve(granted: true, choice: 1)).value
        XCTAssertEqual(prompts.signers.last?.kind, .lock, "no Face ID key needed: the lock key")
        prompts.signers = []
        await vm.answer("ap-1", .approve(granted: false, choice: nil)).value
        await vm.answer("ap-3", .approve(granted: true, choice: 1)).value
        XCTAssertTrue(prompts.signers.isEmpty, "a plain No and a routine Yes ask nothing")
        // the refusal fallback stays: unknown needs, the Mac asks, one prompt
        phone.status = nil
        phone.view = emptyView()
        await vm.refresh()?.value
        phone.answerScript = [.failed(FakePhone.refused("unlock_required"))]
        await vm.answer("ap-9", .approve(granted: true, choice: 1)).value
        XCTAssertEqual(prompts.signers.map(\.kind), [.strong])
    }

    // decision: PHN-125, PHN-126, PHN-519, PHN-511
    func testTheMacsStatusBringsQuestionsTheChatDoesNotHoldYet() async {
        session.unlocked()
        phone.view = emptyView()
        phone.status = phoneStatus(asked("ap-7", needs(unlock: false)), asked("ap-8", needs(unlock: true, can: false)))
        let vm = rig.model()
        await vm.refresh()?.value
        let q = vm.ui.questions
        XCTAssertEqual(q.count, 2)
        XCTAssertTrue(q[0].routine)
        XCTAssertEqual(q[0].options, ["Yes", "No"])
        XCTAssertTrue(q[1].onlyOnMac)
        XCTAssertEqual(vm.ui.work.waiting.map(\.approvalId), ["ap-7", "ap-8"])
    }

    // decision: PHN-13
    func testTheChatOnScreenIsMarkedReadAtItsLatestEntry() async {
        session.unlocked()
        session.foreground(true)
        phone.view = emptyView()
        let vm = rig.model()
        await vm.refresh()?.value
        await settle()
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("mark:") }, "the home screen is not the chat")
        vm.openChat()
        await eventually("marked") { phone.calls.contains("mark:c1") }
    }

    // decision: PHN-113, PHN-121, PHN-129, PHN-11, PHN-516, PHN-521
    func testAKeyPlaceAsksForFaceIdForItsPathAndItsFileGoesWhenTheOwnerIsBack() async throws {
        session.unlocked()
        session.foreground(true)
        phone.folders["~/Documents"] = Folder(path: "~/Documents", parent: "~", locked: false,
                                              entries: [FileEntry(name: "Keys", path: "~/Documents/Keys", dir: true, link: false, size: 0, modified: 0, locked: true)],
                                              total: 1, cut: false, from: 0, next: nil)
        phone.folders["~/Documents/Keys"] = Folder(path: "~/Documents/Keys", parent: "~/Documents", locked: true,
                                                   entries: [FileEntry(name: "id_ed25519", path: "~/Documents/Keys/id_ed25519", dir: false, link: false,
                                                                       size: 10, modified: 0, locked: true)],
                                                   total: 1, cut: false, from: 0, next: nil)
        let vm = rig.model()
        await vm.openFolder("~/Documents").value
        XCTAssertTrue(prompts.signers.isEmpty, "an ordinary folder asks nothing")
        await vm.openEntry(try XCTUnwrap(vm.ui.files.folder?.entries.first))?.value
        XCTAssertEqual(prompts.signers.map(\.kind), [.strong])
        XCTAssertEqual(prompts.whys, [.file])
        XCTAssertEqual(vm.ui.files.folder?.path, "~/Documents/Keys")
        vm.openEntry(try XCTUnwrap(vm.ui.files.folder?.entries.first))
        await eventually("the fetch") { vm.transfers.list.first?.phase == .done }
        let t = try XCTUnwrap(vm.transfers.list.first)
        XCTAssertTrue(t.keyPlace)
        XCTAssertTrue(phone.calls.contains("fetch:~/Documents/Keys/id_ed25519@0:true"))
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: t.local)), phone.fetchBytes)
        // opened in another app: gone when the owner is back in FERB
        vm.handedOut(t.key)
        session.foreground(false)
        await settle()
        session.foreground(true)
        await eventually("the key place's file goes") { vm.transfers.list.isEmpty }
        XCTAssertFalse(FileManager.default.fileExists(atPath: t.local))
        // without the Face ID key (iOS made it unusable): pair again, nothing asked
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .gone), wiped: nil)
        prompts.signers = []
        await vm.openFolder("~/Documents/Keys", keyPlace: true).value
        XCTAssertEqual(vm.ui.files.problem?.key, "needs_pairing_again")
        XCTAssertTrue(prompts.signers.isEmpty)
    }

    // decision: PHN-13, PHN-115
    func testAFetchPausesAndGoesOnFromTheBytesSaved() async throws {
        session.unlocked()
        let vm = rig.model()
        phone.fetchGate = Latch()
        let key = vm.transfers.fetch("~/Downloads/a.bin", name: "a.bin", size: 10, keyPlace: false)
        await eventually("half") { vm.transfers.list.first?.done == 5 }
        XCTAssertEqual(vm.transfers.list.first?.phase, .running)
        vm.pause(key)
        await eventually("paused") { vm.transfers.list.first?.phase == .paused }
        let paused = try XCTUnwrap(vm.transfers.list.first)
        XCTAssertEqual(rig.local.size(paused.local), 5)
        phone.fetchGate = nil
        vm.transfers.resume(key)
        await eventually("done") { vm.transfers.list.first?.phase == .done }
        XCTAssertTrue(phone.calls.contains("fetch:~/Downloads/a.bin@5:false"))
    }

    // decision: PHN-13, PHN-115, PHN-572
    func testBeforeIosSuspendsTheAppATransferPausesAndGoesOnAfter() async throws {
        session.unlocked()
        let vm = rig.model()
        phone.fetchGate = Latch()
        vm.transfers.fetch("~/Downloads/b.bin", name: "b.bin", size: 10, keyPlace: false)
        await eventually("half") { vm.transfers.list.first?.done == 5 }
        await vm.transfers.checkpoint()
        await eventually("paused") { vm.transfers.list.first?.phase == .paused }
        phone.fetchGate = nil
        vm.transfers.resumeAfterBackground()
        await eventually("done") { vm.transfers.list.first?.phase == .done }
        XCTAssertTrue(phone.calls.contains("fetch:~/Downloads/b.bin@5:false"), "on from the bytes saved")
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: vm.transfers.list[0].local)), phone.fetchBytes)
    }

    // decision: PHN-410, PHN-444
    func testOnThePublicRelayAFetchWaitsAndIsTriedOnceMoreWhenFilesCrossAgain() async {
        session.unlocked()
        phone.link = FakePhone.status(path: .publicRelay)
        let vm = rig.model()
        phone.fetchRefusal = .NeedsBetterPath(detail: "This file is available when connected directly or through your relay.", linkCode: nil)
        vm.transfers.fetch("~/a.pdf", name: "a.pdf", size: 10, keyPlace: false)
        await eventually("waiting") { vm.transfers.list.first?.phase == .waitingPath }
        XCTAssertEqual(vm.transfers.list.first?.problem?.key, "err_needs_better_path")
        phone.fetchRefusal = nil
        phone.link = FakePhone.status(path: .ownRelay)
        await eventually("tried once more") { vm.transfers.list.first?.phase == .done }
        XCTAssertEqual(phone.calls.filter { $0.hasPrefix("fetch:~/a.pdf") }.count, 2)
    }

    // decision: PHN-127, PHN-115, PHN-517, PHN-513
    func testPickedFilesAreCopiedHereSentAndRideWithTheNextMessage() async {
        session.unlocked()
        phone.view = emptyView()
        let picks = rig.root.deletingLastPathComponent().appendingPathComponent("picks-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: picks) }
        try? FileManager.default.createDirectory(at: picks, withIntermediateDirectories: true)
        try? Data(repeating: 1, count: 30).write(to: picks.appendingPathComponent("IMG_1.jpg"))
        try? Data(repeating: 2, count: 20).write(to: picks.appendingPathComponent("IMG_2.jpg"))
        let vm = rig.model()
        vm.sendFiles([picks.appendingPathComponent("IMG_1.jpg"), picks.appendingPathComponent("IMG_2.jpg"), picks.appendingPathComponent("gone")],
                     forChat: true)
        await eventually("both sent") { vm.transfers.ready.count == 2 }
        XCTAssertEqual(vm.ui.note?.key, "files_unreadable", "the unreadable one is said")
        XCTAssertEqual(phone.calls.filter { $0.hasPrefix("upload") }, ["upload:IMG_1.jpg:-", "upload:IMG_2.jpg:-"])
        XCTAssertEqual(vm.transfers.ready.map(\.uploadId), ["up-IMG_1.jpg", "up-IMG_2.jpg"])
        let left = (try? FileManager.default.subpathsOfDirectory(atPath: local.outgoing.path)) ?? []
        XCTAssertTrue(left.allSatisfy { !$0.contains(".jpg") }, "the staged copies are gone once sent: \(left)")
        vm.typed("Here are the scans")
        await vm.send().value
        XCTAssertEqual(phone.sentWith.first?.0, "Here are the scans")
        XCTAssertEqual(phone.sentWith.first?.1.count, 2)
        XCTAssertTrue(vm.transfers.ready.isEmpty)
    }

    // decision: PHN-127, PHN-115
    func testAPickedFolderGoesWholeAsOneFolderOnTheMac() async {
        session.unlocked()
        phone.view = emptyView()
        let picks = rig.root.deletingLastPathComponent().appendingPathComponent("Scans-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: picks) }
        write("one", to: picks.appendingPathComponent("2026/a.pdf"))
        write("two", to: picks.appendingPathComponent("b.pdf"))
        let vm = rig.model()
        await vm.sendFolder(picks, forChat: true).value
        await eventually("the folder arrived") { vm.transfers.ready.count == 1 }
        XCTAssertEqual(vm.transfers.ready.first?.uploadId, "g-\(picks.lastPathComponent)", "the folder rides with a message as its group")
        XCTAssertEqual(phone.calls.filter { $0.hasPrefix("upload") }, ["upload:a.pdf:-", "upload:b.pdf:-"])
    }

    // decision: PHN-454, PHN-420, PHN-518
    func testWatchLiveNeverAsksOnThePublicRelayAndStopsInTheBackground() async {
        session.unlocked()
        session.foreground(true)
        phone.link = FakePhone.status(path: .publicRelay)
        let vm = rig.model()
        vm.watchLive("t-1")
        XCTAssertEqual(vm.ui.note?.key, "not_on_shared_relay")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("live:") })
        phone.link = FakePhone.status()
        await settle()
        vm.watchLive("t-1")
        XCTAssertEqual(vm.sheet, .live(taskId: "t-1"))
        XCTAssertTrue(vm.mustSecure, "hidden from screen captures whatever the switch says")
        await eventually("watching") { phone.preview != nil }
        phone.preview?.frame(frame: PreviewFrame(jpeg: nil, seq: 3, state: "working", line: "Reading delivery options", fresh: false, busy: true,
                                                 gone: false, emptyText: nil))
        await eventually("the line") { vm.live.view?.line == "Reading delivery options" }
        phone.preview?.ended(code: "needs_unlock", detail: "Unlock to watch live.")
        await eventually("needs unlock") { vm.live.view?.needsUnlock == true }
        session.foreground(false)
        await eventually("stopped") { phone.previewStopped == 1 }
        XCTAssertNil(vm.sheet)
        XCTAssertFalse(vm.mustSecure)
    }

    // decision: PHN-404, PHN-133, PHN-112, PHN-521
    func testSettingsChangeOneAtATimeEachSignedAfterFaceId() async throws {
        session.unlocked()
        phone.settingsValues = [("sense.window_titles", "true"), ("retention.trace_days", "30"),
                                ("privacy.exclude_apps", #"["Password manager"]"#), ("browsing.owner_profile", #""Default""#)]
        let vm = rig.model()
        // the Mac's window had closed: one unlock, then the read
        phone.settingsRefusal = FakePhone.refused("locked")
        await vm.openSettings().value
        XCTAssertEqual(vm.sheet, .settings)
        XCTAssertEqual(vm.ui.settings?.map(\.spec.key), ["sense.window_titles", "privacy.exclude_apps", "retention.trace_days"])
        XCTAssertEqual(prompts.signers.map(\.kind), [.lock])
        prompts.signers = []
        prompts.whys = []
        await vm.setSwitch("sense.window_titles", on: false).value
        XCTAssertEqual(prompts.signers.map(\.kind), [.strong])
        XCTAssertEqual(prompts.whys, [.setting])
        XCTAssertEqual(vm.ui.settings?.first?.on, false)
        vm.show(.days(key: "retention.trace_days"))
        XCTAssertNil(vm.setDays("retention.trace_days", text: "0"))
        XCTAssertEqual(vm.ui.note?.key, "set_days_invalid")
        await vm.setDays("retention.trace_days", text: "14")?.value
        XCTAssertEqual(phone.calls.last, "set:retention.trace_days=14")
        XCTAssertEqual(vm.ui.settings?.first { $0.spec.key == "retention.trace_days" }?.days, 14)
        XCTAssertEqual(vm.sheet, .settings)
        // a cancelled prompt changes nothing and says nothing
        prompts.cancel = true
        await vm.setSwitch("sense.window_titles", on: true).value
        XCTAssertEqual(vm.ui.settings?.first?.on, false)
    }

    // decision: PHN-13
    func testTasksComeFromTheMacsListAndFromTheChatOffline() async {
        session.unlocked()
        phone.view = emptyView()
        phone.status = phoneStatus(asked("ap-1", needs(unlock: false)))
        phone.tasks = .ok(TaskPage(tasks: [
            TaskRow(taskId: "t2", name: "Compare desks", goal: "Compare the three desks", status: "working", createdAt: nil, updatedAt: nil,
                    answer: "", stepsDone: 2, stepsTotal: 3),
            TaskRow(taskId: "t1", name: "Back up the thesis", goal: "", status: "done", createdAt: nil, updatedAt: nil,
                    answer: "Copied to the drive.", stepsDone: nil, stepsTotal: nil),
            TaskRow(taskId: "t0", name: "Invoice", goal: "", status: "failed", createdAt: nil, updatedAt: nil,
                    answer: "The site asked for a code.", stepsDone: nil, stepsTotal: nil),
        ], more: false))
        let vm = rig.model()
        await vm.refresh()?.value
        vm.choose(.tasks)
        await eventually("the Mac's list") { vm.ui.work.running.map(\.title) == ["Compare desks"] }
        let w = vm.ui.work
        XCTAssertEqual(w.waiting.map(\.approvalId), ["ap-1"])
        XCTAssertEqual(w.running.first?.progress, WorkProgress(done: 2, total: 3))
        XCTAssertEqual(w.earlier.map(\.kind), [.done, .failed])
        // the Mac's list can't be read: the chat's stays
        let vm2 = rig.model()
        phone.tasks = .failed(.Offline(detail: "no", linkCode: nil))
        await vm2.refresh()?.value
        vm2.choose(.tasks)
        await settle()
        XCTAssertNil(vm2.ui.macTasks)
        XCTAssertTrue(vm2.ui.work.running.isEmpty)
    }

    // decision: PHN-133, PHN-522
    func testAFaceIdKeyAddedAfterPairingIsOfferedAndRegistered() async {
        session.unlocked()
        phone.newKeys = PublicKeys(device: nil, lock: nil, strong: "STRONG-PUB")
        phone.mac = MacInfo(name: "Studio Mac", pairedAt: nil, phoneName: nil, uploadFolder: nil)
        let vm = rig.model()
        await vm.unlockMac().value
        XCTAssertTrue(vm.ui.newKeys)
        prompts.signers = []
        prompts.whys = []
        await vm.registerKeys().value
        XCTAssertTrue(phone.calls.contains("keys:STRONG-PUB"))
        XCTAssertEqual(Set(prompts.whys), [.keys])
        XCTAssertFalse(vm.ui.newKeys)
        XCTAssertEqual(vm.ui.note?.key, "keys_registered")
    }

    // decision: PHN-408, PHN-515
    func testForgettingTheMacRemovesEveryFileHere() async {
        session.unlocked()
        write("x", to: local.outgoing.appendingPathComponent("x/a.jpg"))
        let vm = rig.model()
        vm.transfers.fetch("~/a.txt", name: "a.txt", size: 10, keyPlace: false)
        await eventually("fetched") { vm.transfers.list.first?.phase == .done }
        await vm.forget().value
        XCTAssertFalse(exists(rig.root), "everything here goes")
        XCTAssertTrue(vm.transfers.list.isEmpty)
    }

    // decision: GATE-613, SENSE-420
    func testTheMacsRequestForTheExactLocationIsAnsweredOnceYesOrNo() async {
        session.unlocked()
        session.foreground(true)
        phone.location = "loc-1"
        let vm = rig.model()
        await eventually("the request's sheet") { vm.sheet == .location(requestId: "loc-1") }
        await vm.declineLocation("loc-1").value
        XCTAssertNil(vm.sheet)
        XCTAssertTrue(phone.calls.contains { $0.hasPrefix("location:loc-1:false:") })
        XCTAssertEqual(vm.ui.note?.key, "exact_not_sent")
    }
}
