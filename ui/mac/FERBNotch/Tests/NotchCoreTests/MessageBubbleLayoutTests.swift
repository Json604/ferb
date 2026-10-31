import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

@MainActor
final class MessageBubbleLayoutTests: XCTestCase {
    func testShortGreetingPaintsAContentSizedBubbleAtBothPanelWidths() {
        let narrow = measuredBubble("Hi Sam 👋", rowWidth: 380)
        let wide = measuredBubble("Hi Sam 👋", rowWidth: 700)
        XCTAssertGreaterThan(narrow.width, 22)
        XCTAssertLessThan(narrow.width, 200)
        XCTAssertEqual(narrow.width, wide.width, accuracy: 1)
    }

    func testLongReplyWrapsWithinAvailableWidthWithoutChangingPanelSize() {
        let reply = "Your very bottom WhatsApp chat is Group 1 - ABC Class. "
            + "I scrolled through the chat list and checked the final conversation."
        let narrow = measuredBubble(reply, rowWidth: 230)
        let wide = measuredBubble(reply, rowWidth: 600)
        XCTAssertLessThanOrEqual(narrow.width, 230 - 28)
        XCTAssertLessThanOrEqual(wide.width, 600 - 28)
        XCTAssertGreaterThan(narrow.height, wide.height)
    }

    func testRichFieldsStayInsideTheBubbleWidth() {
        let reply = """
        ## Result
        - Checked the final conversation in your WhatsApp chat list.

        ```swift
        let message = "A long literal that stays horizontally scrollable in its own field"
        ```

        | Site | Result |
        | --- | --- |
        | WhatsApp | Checked |
        """
        let size = measuredBubble(reply, rowWidth: 330)
        XCTAssertGreaterThan(size.height, 100)
        XCTAssertLessThanOrEqual(size.width, 330 - 28)
    }

    func testWrappedListKeepsFirstLineBaselineNearTop() {
        let measurement = BaselineMeasurement()
        let host = NSHostingView(rootView: MeasureFirstBaseline(measurement: measurement) {
            MarkdownContent("- This is a long item which wraps across several lines at the available width.")
        }.font(.system(size: 12)).frame(width: 160, height: 300, alignment: .topLeading))
        host.layoutSubtreeIfNeeded()
        XCTAssertGreaterThan(measurement.height, 30)
        XCTAssertGreaterThan(measurement.baseline, 0)
        XCTAssertLessThan(measurement.baseline, 20)
    }

    private func measuredBubble(_ text: String, rowWidth: CGFloat) -> CGSize {
        let measurement = BubbleMeasurement()
        // Measure the exact padded, painted surface, before the row's alignment
        // frame and 28-point trailing gutter; never display a window or capture it.
        let surface = FerbBubble { MarkdownContent(text) }.bubble
        let hosting = NSHostingView(rootView:
            MeasureBubble(measurement: measurement) { surface }
                .font(.system(size: 13))
                .frame(width: rowWidth - 28, height: 1000, alignment: .topLeading))
        hosting.sizingOptions = []
        hosting.frame = CGRect(x: 0, y: 0, width: rowWidth - 28, height: 1000)
        hosting.layoutSubtreeIfNeeded()
        XCTAssertNotEqual(measurement.size, .zero)
        return measurement.size
    }
}

private final class BubbleMeasurement {
    var size: CGSize = .zero
}

private struct MeasureBubble: Layout {
    let measurement: BubbleMeasurement

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let size = subviews[0].sizeThatFits(ProposedViewSize(width: proposal.width, height: nil))
        measurement.size = size
        return size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews[0].place(at: bounds.origin, proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
    }
}

private final class BaselineMeasurement { var height: CGFloat = 0; var baseline: CGFloat = 0 }
private struct MeasureFirstBaseline: Layout {
    let measurement: BaselineMeasurement
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let dimensions = subviews[0].dimensions(in: proposal)
        measurement.height = dimensions.height
        measurement.baseline = dimensions[.firstTextBaseline]
        return CGSize(width: dimensions.width, height: dimensions.height)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews[0].place(at: bounds.origin, proposal: proposal)
    }
}
