import Foundation

/// One thing FERB says in the first-run intro: a line, and an optional smaller line under it. Shown as text (in the
/// notch, or in the bubble beside the orb), never read aloud.
public struct IntroLine: Equatable, Sendable {
    public var text: String
    public var detail: String?
    public init(text: String, detail: String? = nil) {
        self.text = text
        self.detail = detail
    }
}
