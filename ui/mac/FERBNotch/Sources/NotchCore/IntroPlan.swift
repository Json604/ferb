import Foundation

/// FERB.app's first-run intro: its beats, in the spec's order (notes/setup-intro.md).
public enum IntroBeat: Int, CaseIterable, Equatable {
    case meet, signIn, permissions, browsers, identity, tryIt, control, home
}

/// What this install still needs, read by the launch decision.
public struct IntroNeeds: Equatable {
    public var setupDone: Bool
    public var signedIn: Bool
    public var missingPermissions: [String]
    public var unpairedBrowsers: Bool

    public init(setupDone: Bool, signedIn: Bool, missingPermissions: [String], unpairedBrowsers: Bool) {
        self.setupDone = setupDone
        self.signedIn = signedIn
        self.missingPermissions = missingPermissions
        self.unpairedBrowsers = unpairedBrowsers
    }
}

public enum IntroPlan {
    /// The beats this install gets. A finished setup gets none unless replayed (replay is the whole tour). A new install
    /// (nothing open is done yet) gets every beat; an upgrade gets only the beats for what is missing, then home.
    /// FERB's own email (identity) is offered whenever browsers are.
    public static func beats(_ needs: IntroNeeds, replay: Bool) -> [IntroBeat] {
        if replay { return IntroBeat.allCases }
        if needs.setupDone { return [] }
        let fresh = !needs.signedIn && !needs.missingPermissions.isEmpty && needs.unpairedBrowsers
        if fresh { return IntroBeat.allCases }
        var plan: [IntroBeat] = []
        if !needs.signedIn { plan.append(.signIn) }
        if !needs.missingPermissions.isEmpty { plan.append(.permissions) }
        if needs.unpairedBrowsers { plan += [.browsers, .identity] }
        if !plan.isEmpty { plan.append(.home) }
        return plan
    }

    public static func next(after beat: IntroBeat, in plan: [IntroBeat]) -> IntroBeat? {
        plan.first { $0.rawValue > beat.rawValue }
    }
}

/// Where the intro stands across a quit and relaunch.
public struct IntroProgress {
    public static let beatKey = "FERBIntroBeat"
    public static let doneKey = "FERBIntroDone"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults) { self.defaults = defaults }

    public var resumeBeat: IntroBeat? {
        get {
            guard defaults.object(forKey: Self.beatKey) != nil else { return nil }
            return IntroBeat(rawValue: defaults.integer(forKey: Self.beatKey))
        }
        set {
            if let newValue { defaults.set(newValue.rawValue, forKey: Self.beatKey) }
            else { defaults.removeObject(forKey: Self.beatKey) }
        }
    }

    public var done: Bool {
        get { defaults.bool(forKey: Self.doneKey) }
        set { defaults.set(newValue, forKey: Self.doneKey) }
    }

    public mutating func clear() {
        defaults.removeObject(forKey: Self.beatKey)
        defaults.removeObject(forKey: Self.doneKey)
    }
}
