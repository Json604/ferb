import FerbMobile
import Foundation

/// How a setting is shown: a switch, a number of days, or a list changed only on the Mac.
public enum SettingKind: Sendable { case toggle, days, list }

/// One of FERB's settings the phone shows, with FERB.app's own title (its catalog key, `titleKey`).
public struct SettingSpec: Equatable, Sendable {
    public let key: String
    public let titleKey: String
    public let kind: SettingKind
    public var title: String { L.s(titleKey) }

    public init(key: String, titleKey: String, kind: SettingKind) {
        self.key = key
        self.titleKey = titleKey
        self.kind = kind
    }
}

/// A setting's value as read: a switch, days, or the list's entries. (Android's SettingRow; named apart from
/// FerbDesign's SettingRow, the piece that draws it.)
public struct MacSettingRow: Equatable, Sendable {
    public let spec: SettingSpec
    public var on: Bool = false
    public var days: Int? = nil
    public var items: [String] = []

    public init(spec: SettingSpec, on: Bool = false, days: Int? = nil, items: [String] = []) {
        self.spec = spec
        self.on = on
        self.days = days
        self.items = items
    }
}

/// FERB's settings on the phone (PHN-404, PHN-112): what FERB.app's Privacy page shows, in its order. The switch and
/// the day counts change from here, each change signed after one Face ID / Touch ID (PHN-133); the two lists are shown
/// and changed in FERB on the Mac (their editor is the Mac's). browsing.owner_profile is not shown: it names a browser
/// profile only the Mac can pick. (Android: model/MacSettings.kt.)
public enum MacSettings {
    public static let specs: [SettingSpec] = [
        SettingSpec(key: "sense.window_titles", titleKey: "set_window_titles", kind: .toggle),
        SettingSpec(key: "privacy.exclude_apps", titleKey: "set_exclude_apps", kind: .list),
        SettingSpec(key: "privacy.exclude_domains", titleKey: "set_exclude_domains", kind: .list),
        SettingSpec(key: "sense.trail_retention_days", titleKey: "set_trail_days", kind: .days),
        SettingSpec(key: "retention.trace_days", titleKey: "set_trace_days", kind: .days),
        SettingSpec(key: "retention.decision_days", titleKey: "set_decision_days", kind: .days),
        SettingSpec(key: "retention.evidence_days", titleKey: "set_evidence_days", kind: .days),
        SettingSpec(key: "retention.recovery_days", titleKey: "set_recovery_days", kind: .days),
        SettingSpec(key: "retention.resource_days", titleKey: "set_resource_days", kind: .days),
        SettingSpec(key: "retention.operational_days", titleKey: "set_operational_days", kind: .days),
    ]

    /// The rows the Mac gave a value for, in [specs]' order.
    public static func rows(_ values: [SettingValue]) -> [MacSettingRow] {
        var byKey: [String: String] = [:]
        for v in values { byKey[v.key] = v.valueJson }
        return specs.compactMap { spec in byKey[spec.key].map { row(spec, $0) } }
    }

    public static func row(_ spec: SettingSpec, _ json: String) -> MacSettingRow {
        let v = Json.parse(json)
        switch spec.kind {
        case .toggle: return MacSettingRow(spec: spec, on: Json.bool(v) == true)
        case .days: return MacSettingRow(spec: spec, days: Json.number(v).map { Int($0) })
        case .list: return MacSettingRow(spec: spec, items: (v as? [Any])?.compactMap { $0 as? String } ?? [])
        }
    }

    public static func spec(_ key: String) -> SettingSpec? { specs.first { $0.key == key } }
}
