#if canImport(UIKit)
import SwiftUI
import XCTest
@testable import FerbDesign

/// The memory import's live card (kb MEM-456, the owner's MEM-455: "it should show in a good ass UI") in its four
/// states, in the Mac's own words: reading (a bar and what was found so far), putting it together (a quieter detail),
/// two websites waiting for their emails (names, no bar, a try that didn't go through), and a gap while reading.
@MainActor
final class MemoryImportCardTests: XCTestCase {
    static let claudeCode = "Bringing in what Claude Code knows about you"

    /// A: reading what he typed.
    static let reading = MemoryImportRow(key: "run:r1", name: "Claude Code", line: "Reading what you've typed", done: 33, total: 40,
                                         count: "33 of 40", foundLine: "221 things found so far")
    /// B: putting it together.
    static let placing = MemoryImportRow(key: "run:r1", name: "Claude Code", line: "Putting together what I found",
                                         detail: "Many things come up more than once, so I'm joining them up.", done: 3, total: 7,
                                         count: "3 of 7", foundLine: "274 things found")
    /// C: two websites waiting for their emails.
    static let sites = [
        MemoryImportRow(key: "site:chatgpt", name: "ChatGPT", line: "Waiting for ChatGPT's email — asked at 23:40",
                        detail: "I check your email every 15 minutes. It can take a day or two."),
        MemoryImportRow(key: "site:claude", name: "Claude", line: "Waiting for Claude's email — asked yesterday at 23:41",
                        detail: "I check your email every 15 minutes. It can take a day or two.",
                        note: "The last try didn't go through. I'll try again at 00:25."),
    ]
    /// D: reading with a gap.
    static let gap = MemoryImportRow(key: "run:r1", name: "Claude Code", line: "Reading what you've typed", done: 33, total: 40,
                                     count: "33 of 40", foundLine: "221 things found so far",
                                     note: "1 part couldn't be read just now. I'll go back to it next time.")

    private func states(_ hour: Double) -> some View {
        PhoneScreen(moment: .lab(hour), glide: false) {
            VStack(spacing: 12) {
                MemoryImportCard(title: Self.claudeCode, rows: [Self.reading])
                MemoryImportCard(title: Self.claudeCode, rows: [Self.placing])
                MemoryImportCard(title: "Bringing in what ChatGPT and Claude know about you", rows: Self.sites)
                MemoryImportCard(title: Self.claudeCode, rows: [Self.gap])
            }
            .ferbSurface()
            .padding(.horizontal, 16).padding(.top, 24)
            .frame(maxHeight: .infinity, alignment: .top)
        }
        .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
        .stillMotion()
    }

    // decision: MEM-456, PHN-734
    func testTheCardInItsFourStatesOnTheNoonAndNightSkies() {
        Snapshot.assert(states(12.5), size: PhoneInsets.labSize, path: "components/memory-import-noon.png")
        Snapshot.assert(states(22.5), size: PhoneInsets.labSize, path: "components/memory-import-night.png")
    }

    // decision: MEM-456
    func testTheBarIsDoneOfTotalAndNoneWithoutATotal() {
        XCTAssertEqual(Self.reading.fraction, 33.0 / 40.0, accuracy: 1e-12)
        XCTAssertEqual(Self.sites[0].fraction, 0)
        XCTAssertEqual(MemoryImportRow(key: "k", line: "x", done: 9, total: 7).fraction, 1, "never past full")
        // the fill is at least round, and nothing at zero
        let rect = CGRect(x: 0, y: 0, width: 200, height: 6)
        XCTAssertTrue(BarFill(fraction: 0).path(in: rect).isEmpty)
        XCTAssertEqual(BarFill(fraction: 0.01).path(in: rect).boundingRect.width, 6, accuracy: 1e-9)
        XCTAssertEqual(BarFill(fraction: 0.5).path(in: rect).boundingRect.width, 100, accuracy: 1e-9)
    }
}
#endif
