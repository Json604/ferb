// Ported from ~/Desktop/ferb-glass-lab Sources/FerbGlassLab/Words.swift at 23a0fde (LabSource.commit), with the
// lab's words/corpus.json as this package's resource (byte for byte). Kept as the lab wrote them: the corpus's rules,
// Corpus.holds, compare, inHours, fill, the line picked by the day's ordinal, the phase, sunrise and sunset, the moon
// and the fallbacks of HomeTexts. Changed as Android's app did (kb PHN-507; apps/android/app .../model/Words.kt): the
// Moment is what the phone really knows (the sky at its place, FERB's counts, the link, first run, just paired, the
// open before this one and its zone), so the lab's inputs for the weather (paused, PHN-412) and for the owner's habits
// (usual hours, late nights: not kept) are absent and their rules never speak; "first open today" comes from the open
// before (the day starts at 4 am, as the lab's `usual` reads it). The lab's Words scene (its controls) is not carried.

import Foundation

// The words on the phone's home screen, chosen by what the phone knows about the moment: where the sun is, how the
// owner uses the app, and what FERB did while they were away. The words live in words/corpus.json; this file only
// measures the moment and applies the rules. Everything here is computed on the phone and never leaves it.

/// What the phone knows when the app opens.
public struct Moment {
    public var sky: SkyMoment
    public var waiting: Int
    public var done: Int
    public var failed: Int
    public var running: Int
    /// How the phone reaches the Mac: direct, own, shared, offline.
    public var link: String
    public var firstRun: Bool
    public var justPaired: Bool
    /// The open before this one, when the app knows it.
    public var lastOpen: Date?
    /// The time zone at the open before this one.
    public var lastZone: String?

    public init(sky: SkyMoment, waiting: Int = 0, done: Int = 0, failed: Int = 0, running: Int = 0, link: String = "direct",
                firstRun: Bool = false, justPaired: Bool = false, lastOpen: Date? = nil, lastZone: String? = nil) {
        self.sky = sky
        self.waiting = waiting
        self.done = done
        self.failed = failed
        self.running = running
        self.link = link
        self.firstRun = firstRun
        self.justPaired = justPaired
        self.lastOpen = lastOpen
        self.lastZone = lastZone
    }

    var place: SkyPlace { sky.place }
    var date: Date { sky.date }
    var sun: SunPosition { sky.sun }
    var sunDay: SunDay { sky.sunDay }

    var hour: Double {
        let c = place.calendar.dateComponents([.hour, .minute], from: date)
        return Double(c.hour ?? 0) + Double(c.minute ?? 0) / 60
    }

    /// 1 Sunday ... 7 Saturday.
    var weekday: Int { place.calendar.component(.weekday, from: date) }

    /// Where the sun is, in the words the corpus uses.
    public var phase: String {
        let s = sun
        let e = s.elevation, across = SkyView.across(azimuth: s.azimuth, latitude: place.latitude)
        if e < -10 { return "night" }
        if e < -2 { return across < 0 ? "dawn" : "dusk" }
        if e < 3 { return across < 0 ? "sunrise" : "sunset" }
        if across < 0 { return e < 12 || across < -0.15 ? "morning" : "midday" }
        if e < 12 { return "golden-hour" }
        return across < 0.15 ? "midday" : "afternoon"
    }

    var sunsetIn: Int? {
        guard let set = sunDay.sunset, sun.elevation > -0.833, date < set else { return nil }
        return Int(set.timeIntervalSince(date) / 60)
    }

    var sunriseIn: Int? {
        guard let rise = sunDay.sunrise, date < rise else { return nil }
        return Int(rise.timeIntervalSince(date) / 60)
    }

    var weekend: Bool { weekday == 1 || weekday == 7 }
    var season: String { sky.season.rawValue }
    var moon: String {
        let lit = MoonPhase(date: date).lit
        return lit > 0.97 ? "full" : lit < 0.03 ? "new" : ""
    }

    /// Hours since the open before this one.
    var awayHours: Double? { lastOpen.map { date.timeIntervalSince($0) / 3600 } }

    /// The first open since 4 am today (before 4 am, since 4 am yesterday).
    var firstToday: Bool? {
        guard let lastOpen else { return nil }
        let cal = place.calendar
        var fourAm = cal.startOfDay(for: date).addingTimeInterval(4 * 3600)
        if cal.component(.hour, from: date) < 4 { fourAm = cal.date(byAdding: .day, value: -1, to: fourAm) ?? fourAm }
        return lastOpen < fourAm
    }

    /// The values the rules test, by name. What the phone does not measure is absent, so a rule on it never holds.
    public var values: [String: Any] {
        var v: [String: Any] = [
            "phase": phase, "hour": hour, "weekend": weekend, "waiting": waiting, "done": done, "failed": failed,
            "running": running, "link": link, "firstRun": firstRun, "justPaired": justPaired,
        ]
        v["weekday"] = weekday
        v["season"] = season
        v["moon"] = moon
        if let sunsetIn { v["sunsetIn"] = sunsetIn }
        if let sunriseIn { v["sunriseIn"] = sunriseIn }
        if let awayHours { v["awayHours"] = awayHours }
        if let firstToday { v["firstToday"] = firstToday }
        if let lastZone { v["newPlace"] = lastZone != place.zone }
        return v
    }

    /// What the placeholders in a line become.
    public var fills: [String: String] {
        let day = sunDay
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = place.calendar.timeZone
        f.dateFormat = "EEEE"
        let weekdayName = f.string(from: date)
        var out: [String: String] = [
            "sunset": day.sunset.map(place.clock) ?? "", "sunrise": day.sunrise.map(place.clock) ?? "",
            "sunsetIn": sunsetIn.map(String.init) ?? "", "sunriseIn": sunriseIn.map(String.init) ?? "",
            "done": "\(done)", "waiting": "\(waiting)", "failed": "\(failed)", "running": "\(running)",
            "day": weekdayName, "time": place.clock(date), "season": season,
        ]
        if let lastOpen {
            out["lastOpen"] = place.clock(lastOpen)
            out["awayDays"] = "\(Int(date.timeIntervalSince(lastOpen) / 86400))"
        }
        return out
    }

    /// The day the line is picked by: Calendar's ordinal of the day in the era (0001-01-01 is day 1).
    var dayNumber: Int { place.calendar.ordinality(of: .day, in: .era, for: date) ?? 0 }
}

/// One rule from the corpus.
public struct WordRule: Identifiable {
    public let id: String
    public let priority: Double
    let when: [String: Any]
    public let lines: [String]
}

/// The corpus and how a line is picked.
public final class Corpus: @unchecked Sendable {
    public static let slots = ["greeting", "subline", "chat", "away", "placeholder"]

    /// The lab's words/corpus.json, as this package carries it.
    /// Where the package keeps the lab's corpus.json.
    public static var bundledURL: URL? { Bundle.module.url(forResource: "corpus", withExtension: "json") }

    public static let bundled: Corpus = {
        guard let url = bundledURL,
              let data = try? Data(contentsOf: url), let c = try? Corpus(data: data) else { return Corpus(rules: [:]) }
        return c
    }()

    public let rules: [String: [WordRule]]

    init(rules: [String: [WordRule]]) { self.rules = rules }

    public convenience init(data: Data) throws {
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let slots = root["slots"] as? [String: [[String: Any]]] else { throw NSError(domain: "corpus", code: 1) }
        var out: [String: [WordRule]] = [:]
        for (slot, list) in slots {
            let parsed = list.enumerated().map { (i, r) in
                (i, WordRule(id: r["id"] as? String ?? "?", priority: (r["priority"] as? NSNumber)?.doubleValue ?? 0,
                             when: r["when"] as? [String: Any] ?? [:], lines: r["lines"] as? [String] ?? []))
            }
            // Highest priority first; equal ones keep the file's order.
            out[slot] = parsed.sorted { $0.1.priority != $1.1.priority ? $0.1.priority > $1.1.priority : $0.0 < $1.0 }.map(\.1)
        }
        self.init(rules: out)
    }

    /// Every rule of a slot whose conditions all hold, highest priority first.
    public func matching(_ slot: String, _ m: Moment) -> [WordRule] {
        let v = m.values
        return (rules[slot] ?? []).filter { rule in rule.when.allSatisfy { Corpus.holds($0.key, $0.value, v[$0.key]) } }
    }

    /// The line shown: from the winning rule, the same one all day and a different one the next.
    public func line(_ slot: String, _ m: Moment) -> (rule: WordRule, index: Int, text: String)? {
        guard let rule = matching(slot, m).first, !rule.lines.isEmpty else { return nil }
        let index = (m.dayNumber + slot.count * 7) % rule.lines.count
        return (rule, index, Corpus.fill(rule.lines[index], m.fills))
    }

    public static func fill(_ line: String, _ fills: [String: String]) -> String {
        var out = line
        for (k, v) in fills { out = out.replacingOccurrences(of: "{\(k)}", with: v) }
        return out
    }

    public static func holds(_ key: String, _ want: Any, _ have: Any?) -> Bool {
        guard let have else { return false }
        if key == "hour", let range = want as? String, let h = have as? Double { return inHours(range, h) }
        if let list = want as? [String], let s = have as? String { return list.contains(s) }
        if let s = want as? String, let x = number(have) { return compare(s, x) }
        if let s = want as? String, let hs = have as? String { return s == hs }
        if isBool(want) || have is Bool { return (want as? Bool) == (have as? Bool) && isBool(want) && have is Bool }
        if let n = number(want), let x = number(have) { return n == x }
        return false
    }

    /// JSON's true and false arrive as NSNumber, like its numbers; only these two are booleans.
    private static func isBool(_ v: Any) -> Bool {
        if let n = v as? NSNumber { return CFGetTypeID(n) == CFBooleanGetTypeID() }
        return v is Bool
    }

    private static func number(_ v: Any) -> Double? {
        if isBool(v) { return nil }
        if let i = v as? Int { return Double(i) }
        if let d = v as? Double { return d }
        if let n = v as? NSNumber { return n.doubleValue }
        return nil
    }

    private static func compare(_ s: String, _ x: Double) -> Bool {
        let t = s.trimmingCharacters(in: .whitespaces)
        for op in [">=", "<=", ">", "<"] where t.hasPrefix(op) {
            guard let n = Double(t.dropFirst(op.count)) else { return false }
            switch op {
            case ">=": return x >= n
            case "<=": return x <= n
            case ">": return x > n
            default: return x < n
            }
        }
        let parts = t.split(separator: "-").compactMap { Double($0) }
        if parts.count == 2 { return x >= parts[0] && x <= parts[1] }
        return Double(t) == x
    }

    private static func inHours(_ range: String, _ h: Double) -> Bool {
        let p = range.split(separator: "-").compactMap { Double($0) }
        guard p.count == 2 else { return false }
        return p[0] <= p[1] ? (h >= p[0] && h < p[1]) : (h >= p[0] || h < p[1])
    }
}

/// The home screen's words for a moment.
public struct HomeTexts: Equatable {
    public var greeting: String
    public var subline: String
    public var away: String
    public var placeholder: String
    /// FERB's one short, laid-back line of the moment (the weather, the day), or nothing.
    public var chat: String?

    public init(greeting: String, subline: String, away: String = "While you were away", placeholder: String = "Ask FERB",
                chat: String? = nil) {
        self.greeting = greeting
        self.subline = subline
        self.away = away
        self.placeholder = placeholder
        self.chat = chat
    }

    public init(_ m: Moment, corpus: Corpus = .bundled) {
        func say(_ slot: String, _ fallback: String) -> String { corpus.line(slot, m)?.text ?? fallback }
        greeting = say("greeting", "Hello")
        subline = say("subline", "")
        away = say("away", "While you were away")
        placeholder = say("placeholder", "Ask FERB")
        chat = corpus.line("chat", m)?.text
    }
}
