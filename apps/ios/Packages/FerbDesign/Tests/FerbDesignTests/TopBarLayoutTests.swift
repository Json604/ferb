#if canImport(UIKit)
import SwiftUI
import XCTest
@testable import FerbDesign

/// The top bar giving way (the owner, 2026-11-23: "jsut pack them in a good looking hamburger icon if theres no space
/// ... adjust based on that, contracat if really no space"): normal while every unchosen tab keeps 36; folded (the
/// temporary bubble and the profile button in one menu button) when it doesn't; contracted (the chosen tab drops its
/// name) when even that is too tight. The bar's width is the window less the safe area's sides and its 16 margins.
@MainActor
final class TopBarLayoutTests: XCTestCase {
    private func layout(_ screen: CGFloat, left: CGFloat = 0, right: CGFloat = 0, chosen: PhoneTab = .chat, scale: CGFloat = 1,
                        temporary: Bool = true) -> TopBarLayout {
        TopBarLayout.of(width: screen - 32 - left - right, nameWidth: ceil(TopBar.nameWidth(chosen) * scale), temporary: temporary)
    }

    // decision: PHN-820, PHN-801
    func testEachStepGivesWayAtItsThreshold() {
        // wide: everything as built (PHN-819's bubble beside the profile button)
        XCTAssertEqual(layout(402).mode, .normal)
        XCTAssertEqual(layout(402).tabWidth, 46)
        XCTAssertEqual(layout(375).mode, .normal)
        XCTAssertEqual(layout(375).tabWidth, 46)
        // the last width that still fits both round buttons on the chat tab, and the first that folds
        let chat = TopBarLayout.chosenWidth(name: TopBar.nameWidth(.chat))
        let fits = 36 * CGFloat(PhoneTab.allCases.count - 1) + chat + TopBarLayout.pillPadding + 2 * TopBarLayout.button
        XCTAssertEqual(TopBarLayout.of(width: fits, nameWidth: TopBar.nameWidth(.chat), temporary: true).mode, .normal)
        XCTAssertEqual(TopBarLayout.of(width: fits - 1, nameWidth: TopBar.nameWidth(.chat), temporary: true).mode, .folded)
        // narrow: folded, the tabs at 36 or more again
        let narrow = layout(320)
        XCTAssertEqual(narrow.mode, .folded)
        XCTAssertTrue(narrow.menu && narrow.named)
        XCTAssertGreaterThanOrEqual(narrow.tabWidth, 36)
        // even folded too tight: contracted (icon only in the pill), still the menu button
        let tight = layout(320, left: 59, right: 59)
        XCTAssertEqual(tight.mode, .contracted)
        XCTAssertTrue(tight.menu)
        XCTAssertFalse(tight.named)
    }

    // decision: PHN-820
    func testABigCutoutOnOneSideFolds() {
        XCTAssertEqual(layout(375).mode, .normal)
        XCTAssertEqual(layout(375, left: 60).mode, .folded)
    }

    /// The name as measured decides: a name drawn 2.5 to 3 times as wide folds or contracts the bar.
    // decision: PHN-820
    func testALargerNameFoldsOrContracts() {
        XCTAssertEqual(layout(375, scale: 2).mode, .normal)
        XCTAssertEqual(layout(375, scale: 2.5).mode, .folded)
        XCTAssertEqual(layout(375, scale: 3).mode, .folded)
        XCTAssertEqual(layout(320, scale: 2.5).mode, .contracted)
    }

    /// Off the chat tab there is no bubble: nothing folds; where the profile button alone doesn't fit, the name goes.
    // decision: PHN-820
    func testOtherTabsNeverFold() {
        XCTAssertEqual(layout(402, chosen: .tasks, temporary: false).mode, .normal)
        XCTAssertEqual(layout(320, chosen: .tasks, temporary: false).mode, .normal)
        let narrow = layout(320, left: 59, right: 59, chosen: .tasks, temporary: false)
        XCTAssertEqual(narrow.mode, .contracted)
        XCTAssertFalse(narrow.menu)
        XCTAssertEqual(layout(320, chosen: .chat, temporary: false).mode, .normal)
    }

    private func toggle(_ on: Bool) -> TemporaryToggle {
        TemporaryToggle(on: on, label: on ? "End temporary chat" : "Temporary chat", menuLabel: "More") {}
    }

    private func bars(_ width: CGFloat, height: CGFloat, insets: PhoneInsets) -> some View {
        PhoneScreen(moment: .lab(12.5), insets: insets, glide: false) {
            VStack(spacing: 18) {
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: toggle(false))
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: toggle(true))
                TopBar(chosen: .files, waiting: .tasks, onTab: { _ in }, onProfile: {})
                TopBar(chosen: .chat, waiting: nil, onTab: { _ in }, onProfile: {}, temporary: toggle(true), menuOpen: true)
                Spacer()
            }
            .environment(\.connection, Connection(mark: .direct, label: "Connected", onClick: {}))
        }
        .frame(width: width, height: height)
        .stillMotion()
    }

    /// At 320 the chat tab's bar folds (off, and on with the bubble's mark on the menu button), Files keeps its name,
    /// and the open menu; at 375 everything fits as built; at 402 with 59 on each side (a landscape-like cutout) the chat
    /// tab's bar folds.
    // decision: PHN-820
    func testGoldensTheBarGivingWay() {
        Snapshot.assert(bars(320, height: 568, insets: PhoneInsets(top: 20, bottom: 0)), size: CGSize(width: 320, height: 568),
                        path: "components/topbar-fold-320.png")
        Snapshot.assert(bars(375, height: 667, insets: PhoneInsets(top: 20, bottom: 0)), size: CGSize(width: 375, height: 667),
                        path: "components/topbar-fold-375.png")
        Snapshot.assert(bars(402, height: 874, insets: PhoneInsets(top: 20, bottom: 0, left: 59, right: 59)), size: CGSize(width: 402, height: 874),
                        path: "components/topbar-fold-402-sides.png")
    }
}
#endif
