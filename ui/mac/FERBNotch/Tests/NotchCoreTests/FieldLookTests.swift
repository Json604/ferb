import AppKit
import NotchCore
import SwiftUI
import XCTest
@testable import FERBNotch

/// Past-bug guard (TEST-503): AppKit's own bezel on a default-style secure field drew a near-black box on FERB's dark
/// glass, focused or not (owner 2026-11-09: "what is this black box UI when i click on text box???"). Every field
/// FERB shows has FERB's one field look (APP-896): focused, it is never darker than the panel around it. A field in a
/// row of the chat's transcript list (an NSTableView) had the table turn its field editor's near-black background on
/// while it was edited (owner 2026-11-12: "the black input box color is still there"; APP-897). The windows here are
/// never ordered front.
@MainActor
final class FieldLookTests: XCTestCase {
    /// FERB's frosted glass as one flat grey, lighter than AppKit's near-black text background (0.12) so that shows
    private static let panel = 0.3

    // decision: APP-896
    func testAFocusedFieldIsNeverDarkerThanThePanelAroundIt() async throws {
        let cases: [(String, AnyView)] = [
            ("secure field", AnyView(SecureField("Paste your API key", text: .constant("")).ferbField())),
            ("text field", AnyView(TextField("Model id", text: .constant("")).ferbField())),
        ]
        for (name, field) in cases {
            let (inside, around) = try await render(field)
            XCTAssertGreaterThanOrEqual(inside.darkest, around - 0.01, "\(name): no part of it is darker than the panel")
            XCTAssertGreaterThan(inside.mean, around + 0.02, "\(name): its light fill shows")
        }
    }

    // decision: APP-897
    func testAFieldInAChatRowIsNeverDarkerThanThePanelWhileTypedIn() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("FieldLook"))
        let approval = DaemonApproval(["approval_id": "pay", "kind": "owner_question", "reason": "Pay $12?",
                                       "options": ["Pay", "Don't pay"], "corrects": true, "from_task": true])!
        let cases: [(String, AnyView)] = [
            ("a safety check's correction box", AnyView(ApprovalView(model: model, approval: approval))),
            ("a multi-line field", AnyView(TextField("In your words…", text: .constant(""), axis: .vertical)
                .font(.system(size: 12)).ferbField(inset: 7).padding(20))),
        ]
        for (name, row) in cases {
            let (inside, around) = try await render(row, inTranscript: true)
            XCTAssertGreaterThanOrEqual(inside.darkest, around - 0.01, "\(name): no part of it is darker than the panel")
            XCTAssertGreaterThan(inside.mean, around + 0.02, "\(name): its light fill shows")
        }
    }

    /// The field focused and typed in, in a dark window, drawn: the luminance inside the field's box (its control and
    /// most of its inset) and of the panel beside it. `inTranscript`: the view is a row of the chat's transcript list.
    private func render(_ field: AnyView, inTranscript: Bool = false) async throws
        -> (inside: (darkest: Double, mean: Double), around: Double) {
        let size = CGSize(width: 400, height: inTranscript ? 340 : 120)
        let content = Panel(frame: CGRect(origin: .zero, size: size))
        var list: TranscriptList?
        let host: NSView
        if inTranscript {
            let transcript = TranscriptList(marks: AnchorMarks())
            transcript.content = { row in
                guard case .question = row else { return AnyView(EmptyView()) }
                return AnyView(field.environment(\.colorScheme, .dark))
            }
            transcript.scroll.frame = CGRect(x: 40, y: 0, width: size.width - 40, height: size.height)
            host = transcript.scroll
            list = transcript
        } else {
            host = NSHostingView(rootView: field.frame(width: 240).frame(width: 320, height: 120)
                .environment(\.colorScheme, .dark))
            host.frame = CGRect(x: 40, y: 0, width: 320, height: 120)
        }
        content.addSubview(host)
        let window = NSWindow(contentRect: CGRect(origin: .zero, size: size), styleMask: .borderless,
                              backing: .buffered, defer: true)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = content                                // never ordered front or captured
        list?.update(rows: [.top, .question(DaemonApproval(["approval_id": "row"])!), .bottom])
        for _ in 0..<4 {
            content.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let control = try XCTUnwrap(textFields(in: content).first, "the field is an AppKit text field")
        XCTAssertTrue(window.makeFirstResponder(control))
        (control.currentEditor() as? NSTextView)?.insertText("typing here", replacementRange: NSRange(location: 0, length: 0))
        for _ in 0..<3 {
            content.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let rep = try XCTUnwrap(content.bitmapImageRepForCachingDisplay(in: content.bounds))
        content.cacheDisplay(in: content.bounds, to: rep)
        let scale = CGFloat(rep.pixelsWide) / content.bounds.width
        let box = control.convert(control.bounds, to: content).insetBy(dx: -4, dy: -4)
        func pixels(_ rect: CGRect) -> [Double] {
            let top = content.isFlipped ? rect.minY : content.bounds.height - rect.maxY     // bitmap rows run top-down
            var found: [Double] = []
            for y in stride(from: Int(top * scale), to: Int((top + rect.height) * scale), by: 1) {
                for x in stride(from: Int(rect.minX * scale), to: Int(rect.maxX * scale), by: 1) {
                    guard let colour = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                    found.append(0.2126 * colour.redComponent + 0.7152 * colour.greenComponent
                                 + 0.0722 * colour.blueComponent)
                }
            }
            return found
        }
        let inside = pixels(box)
        let around = pixels(CGRect(x: 4, y: 4, width: 20, height: 20))
        withExtendedLifetime((window, list)) {}
        return ((inside.min() ?? 0, inside.reduce(0, +) / Double(max(inside.count, 1))),
                around.reduce(0, +) / Double(max(around.count, 1)))
    }

    /// The panel's grey, drawn (a layer's own colour is not in a cached display).
    private final class Panel: NSView {
        override func draw(_ dirtyRect: NSRect) {
            NSColor(white: FieldLookTests.panel, alpha: 1).setFill()
            dirtyRect.fill()
        }
    }

    private func textFields(in view: NSView) -> [NSTextField] {
        ((view as? NSTextField).map { [$0] } ?? []) + view.subviews.flatMap(textFields)
    }
}
