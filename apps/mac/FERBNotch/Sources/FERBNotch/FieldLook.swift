import SwiftUI

/// FERB's one look for a text or secure field (APP-896): no bezel of AppKit's, a light rounded fill the panel shows
/// through, the question cards' look (ApprovalView). AppKit's own bezel (a default or `.roundedBorder` field) draws a
/// near-black box on FERB's dark glass whether focused or not (owner 2026-11-09: "what is this black box UI when i
/// click on text box???"). A field keeps its own behaviour (submit, focus, secrecy); only its chrome is this.
struct FerbField: ViewModifier {
    static let fill = Color.white.opacity(0.07)
    static let corner: CGFloat = 8
    var inset: CGFloat = 8

    func body(content: Content) -> some View {
        content.textFieldStyle(.plain).padding(inset)
            .background(Self.fill, in: RoundedRectangle(cornerRadius: Self.corner))
    }
}

extension View {
    /// FERB's field look (FerbField) on a TextField or SecureField.
    func ferbField(inset: CGFloat = 8) -> some View { modifier(FerbField(inset: inset)) }
}
