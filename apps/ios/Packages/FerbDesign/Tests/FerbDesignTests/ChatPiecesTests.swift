#if canImport(UIKit)
import SwiftUI
import XCTest
@testable import FerbDesign

/// The chat's new pieces as the owner sees them (kb PHN-626..PHN-628; Android's ChatStateShotsTest): the waiting look
/// while the first page comes, a day's start, the "new message" pill, and the home screen's way back to earlier days.
@MainActor
final class ChatPiecesTests: XCTestCase {
    // decision: PHN-696, PHN-628
    func testTheWaitingLookADaysStartAndThePill() {
        let view = PhoneScreen(moment: .lab(22.5), glide: false) {
            VStack(spacing: 16) {
                ChatWaiting("Getting your chat from your Mac…")
                DayDivider("Yesterday")
                DayDivider("Today")
                NewMessagePill("New message") {}
                Spacer()
            }
            .padding(.top, 60)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "components/chat-pieces-night.png")
    }

    // decision: PHN-626
    func testANewDaysHomeScreenOffersTheEarlierMessages() {
        let view = PhoneScreen(moment: .lab(22.5), glide: false) {
            HomePage(insets: .lab, earlier: "Earlier messages", onEarlier: {})
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "screens/home-earlier-night.png")
    }

    // MARK: the status line (kb PHN-660, PHN-661) and the way back to the latest messages (PHN-662, PHN-663)

    private func statusStates(_ hour: Double) -> some View {
        PhoneScreen(moment: .lab(hour), glide: false) {
            VStack(spacing: 14) {
                StatusLine("Reaching your Mac…", tone: .working)
                StatusLine("Still reaching your Mac…", tone: .working)
                StatusLine("FERB may use your Mac's screen until you're back", tone: .info)
                StatusLine("Can't reach your Mac. Messages wait for it.", tone: .attention)
                FerbBubble("Your desk arrives Thursday.").frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16)
                Spacer()
            }
            .padding(.top, 90)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
    }

    // decision: PHN-660, PHN-661
    func testTheStatusLineAtDawnNoonAndNight() {
        Snapshot.assert(statusStates(6.1), size: PhoneInsets.labSize, path: "components/status-line-dawn.png")
        Snapshot.assert(statusStates(12.5), size: PhoneInsets.labSize, path: "components/status-line-noon.png")
        Snapshot.assert(statusStates(22.5), size: PhoneInsets.labSize, path: "components/status-line-night.png")
    }

    // decision: PHN-662, PHN-663
    func testTheWayBackAndTheDayHeIsIn() {
        let view = PhoneScreen(moment: .lab(22.5), glide: false) {
            VStack(spacing: 18) {
                StickyDay("Yesterday")
                FerbBubble("Yesterday, line 10.").frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16)
                JumpPill("Today", label: "Go to the latest messages") {}
                JumpPill(nil, label: "Go to the latest messages") {}
                NewMessagePill("New message") {}
                Spacer()
            }
            .padding(.top, 90)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "components/chat-way-back-night.png")
    }

    // decision: PHN-662
    func testWhatTheWayBackShows() {
        XCTAssertEqual(ChatJump.of(atEnd: true, unseen: true, shownDay: 9, endDay: 10), .none)
        XCTAssertEqual(ChatJump.of(atEnd: false, unseen: true, shownDay: 9, endDay: 10), .newMessage)
        XCTAssertEqual(ChatJump.of(atEnd: false, unseen: false, shownDay: 9, endDay: 10), .day)
        XCTAssertEqual(ChatJump.of(atEnd: false, unseen: false, shownDay: 10, endDay: 10), .latest)
        XCTAssertEqual(ChatJump.of(atEnd: false, unseen: false, shownDay: nil, endDay: nil), .latest)
        // in the earlier days: the way back to today, at their end too (kb PHN-698)
        XCTAssertEqual(ChatJump.of(atEnd: true, unseen: false, shownDay: 9, endDay: 9, earlier: true), .today)
        XCTAssertEqual(ChatJump.of(atEnd: false, unseen: false, shownDay: 8, endDay: 9, earlier: true), .today)
    }

    // MARK: files in the message box and a message with files on its way (kb PHN-690, PHN-691)

    private func chip(_ name: String, _ size: Int64, remove: Bool = true) -> AttachmentChip {
        AttachmentChip(name, detail: ByteCountFormatter.string(fromByteCount: size, countStyle: .file),
                       removeLabel: remove ? "Remove \(name)" : nil, onRemove: remove ? {} : nil)
    }

    // decision: PHN-690, PHN-772
    func testFilesWaitInTheMessageBox() {
        let view = PhoneScreen(moment: .lab(12.5), glide: false) {
            VStack(spacing: 24) {
                Spacer()
                // the app's own row: compact chips that wrap (owner kb PHN-772); the words as typed (no text field drawn)
                PhoneComposer(placeholder: "Add a note or just send", onSend: {}, onAttach: {},
                              attachments: AnyView(AttachmentRow { chip("IMG-20261118-WA0011.jpg", 2_400_000) }))
                PhoneComposer(placeholder: "Add a note or just send", typed: "Rename these by date", onSend: {}, onAttach: {},
                              attachments: AnyView(AttachmentRow {
                                  chip("Lease agreement.pdf", 1_100_000)
                                  chip("IMG_2041.jpg", 3_200_000)
                                  chip("Budget 2027.xlsx", 84_000)
                              }))
            }
            .padding(.horizontal, 16).padding(.bottom, 40)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "components/composer-files-noon.png")
    }

    // decision: PHN-691
    func testMessagesWithFilesOnTheirWay() {
        let view = PhoneScreen(moment: .lab(22.5), glide: false) {
            VStack(spacing: 18) {
                FilesMessage(files: [(name: "IMG_2041.jpg", detail: "3.2 MB", thumbnail: nil), (name: "IMG_2042.jpg", detail: "2.9 MB", thumbnail: nil)],
                             text: "Here are the photos from the site", status: "Sending to your Mac · 44%", bad: false, actions: [("Cancel", {})])
                FilesMessage(files: [(name: "Lease agreement.pdf", detail: "1.1 MB", thumbnail: nil)], text: "And the lease",
                             status: "Files wait for a direct connection or your own relay", bad: false,
                             actions: [("Send text only", {}), ("Remove", {})])
                FilesMessage(files: [(name: "Scan 3.pdf", detail: "640 KB", thumbnail: nil)], text: "",
                             status: "Not sent: FERB closed before it was sent", bad: true, actions: [("Try again", {}), ("Remove", {})])
                Spacer()
            }
            .padding(.horizontal, 16).padding(.top, 80)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
        Snapshot.assert(view, size: PhoneInsets.labSize, path: "components/files-messages-night.png")
    }

    // MARK: the connection control and its sheet (kb PHN-751, PHN-752)

    private func controls(_ hour: Double) -> some View {
        PhoneScreen(moment: .lab(hour), glide: false) {
            VStack(spacing: 22) {
                ForEach([LinkMark.connecting, .direct, .ownRelay, .sharedRelay, .unreachable], id: \.self) { m in
                    TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {})
                        .environment(\.connection, Connection(mark: m, label: "\(m)", onClick: {}))
                }
                Spacer()
            }
            .padding(.top, 20)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
    }

    // decision: PHN-751
    func testTheConnectionControlAtDawnNoonAndNight() {
        Snapshot.assert(controls(6.1), size: PhoneInsets.labSize, path: "components/connection-dawn.png")
        Snapshot.assert(controls(12.5), size: PhoneInsets.labSize, path: "components/connection-noon.png")
        Snapshot.assert(controls(22.5), size: PhoneInsets.labSize, path: "components/connection-night.png")
    }

    private func sheet(_ mark: LinkMark, _ state: String, path: String?, reason: String?, relay: RelayCopy?, tryAgain: Bool) -> some View {
        PhoneScreen(moment: .lab(22.5), glide: false) {
            ConnectionSheet(mac: "Your Mac", mark: mark, state: state, path: path, reason: reason, relay: relay,
                            tryAgain: tryAgain ? "Try again" : nil, onTryAgain: {}, more: "More on the Mac tab", onMore: {}, onClose: {})
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
    }

    // decision: PHN-752
    func testTheConnectionSheet() {
        Snapshot.assert(sheet(.connecting, "Connecting to your Mac…", path: nil, reason: nil, relay: nil, tryAgain: true),
                        size: PhoneInsets.labSize, path: "components/connection-sheet-connecting.png")
        Snapshot.assert(sheet(.sharedRelay, "Connected", path: "Through the shared relay", reason: nil, relay: LabSample.relay, tryAgain: false),
                        size: PhoneInsets.labSize, path: "components/connection-sheet-shared.png")
        Snapshot.assert(sheet(.unreachable, "Can't reach your Mac", path: nil, reason: "Your Mac didn't answer. It may be asleep or off the network.",
                              relay: nil, tryAgain: true), size: PhoneInsets.labSize, path: "components/connection-sheet-unreachable.png")
    }
}
#endif
