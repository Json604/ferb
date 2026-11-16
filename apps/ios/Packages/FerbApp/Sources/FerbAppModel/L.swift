import FerbCore
import Foundation

/// The app's words: the one string catalog (Resources/Localizable.xcstrings, written by apps/ios/tools/strings.py
/// from the Android app's strings.xml, the same names; kb PHN-567).
public enum L {
    /// The words under `key`, with Android's %1$@ / %1$lld arguments filled.
    public static func s(_ key: String, _ args: CVarArg...) -> String {
        let text = Bundle.module.localizedString(forKey: key, value: "⟦\(key)⟧", table: "Localizable")
        return args.isEmpty ? text : String(format: text, locale: Locale.current, arguments: args)
    }

    /// A line where Android says "your fingerprint": %@ becomes this phone's own kind (Face ID, Touch ID, Optic ID,
    /// or the passcode when there is no biometric).
    public static func bio(_ key: String, _ kind: Biometry = .current) -> String {
        s(key).replacingOccurrences(of: "%@", with: bioWord(kind))
    }

    /// A plural (Android's <plurals>): the form for `count`, with this phone's own kind as the second argument where
    /// Android says "your fingerprint" (the catalog's %2$@).
    public static func plural(_ key: String, _ count: Int, _ kind: Biometry = .current) -> String {
        String(format: s(key), locale: Locale.current, count, bioWord(kind))
    }

    public static func bioWord(_ kind: Biometry = .current) -> String {
        switch kind {
        case .faceID: s("bio_face")
        case .touchID: s("bio_touch")
        case .opticID: s("bio_optic")
        case .none: s("bio_passcode")
        }
    }

    /// Whether the catalog has `key` (tests: every key the app names exists).
    public static func has(_ key: String) -> Bool {
        Bundle.module.localizedString(forKey: key, value: "\u{0}", table: "Localizable") != "\u{0}"
    }
}
