import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

/// Past-bug guard (TEST-503): AppKit's own bezel on a default-style secure field drew a near-black box on FERB's dark
/// glass, focused or not (owner 2026-11-09: "what is this black box UI when i click on text box???"). Every field
/// FERB shows has FERB's one field look (APP-896): focused, it is never darker than the panel around it. The window
/// here is never ordered front.
@MainActor
final class FieldLookTests: XCTestCase {
    private static let panel = 0.12                                   // FERB's dark glass, as one flat grey

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

    /// The field focused in a dark window, drawn: the luminance inside the field's box (its control and most of its
    /// inset) and of the panel beside it.
    private func render(_ field: AnyView) async throws -> (inside: (darkest: Double, mean: Double), around: Double) {
        let root = ZStack {
            Color(white: Self.panel)
            field.frame(width: 240)
        }
        .frame(width: 320, height: 120)
        .environment(\.colorScheme, .dark)
        let host = NSHostingView(rootView: root)
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 320, height: 120),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host                                   // never ordered front or captured
        for _ in 0..<3 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let control = try XCTUnwrap(textFields(in: host).first, "the field is an AppKit text field")
        XCTAssertTrue(window.makeFirstResponder(control))
        for _ in 0..<2 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let scale = CGFloat(rep.pixelsWide) / host.bounds.width
        let box = control.convert(control.bounds, to: host).insetBy(dx: -4, dy: -4)
        func pixels(_ rect: CGRect) -> [Double] {
            let top = host.isFlipped ? rect.minY : host.bounds.height - rect.maxY       // bitmap rows run top-down
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
        withExtendedLifetime(window) {}
        return ((inside.min() ?? 0, inside.reduce(0, +) / Double(max(inside.count, 1))),
                around.reduce(0, +) / Double(max(around.count, 1)))
    }

    private func textFields(in view: NSView) -> [NSTextField] {
        ((view as? NSTextField).map { [$0] } ?? []) + view.subviews.flatMap(textFields)
    }
}
