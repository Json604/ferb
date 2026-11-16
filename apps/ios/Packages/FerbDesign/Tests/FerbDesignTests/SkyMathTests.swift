import Foundation
import XCTest
@testable import FerbDesign

/// The sky's arithmetic against Android's sky-golden.json (apps/android/tools/sky_golden.swift: the lab's own
/// formulas re-evaluated for fixed times and places): the iPhone's copy must give the same numbers to 1e-9, so both
/// phones draw the same sky.
final class SkyMathTests: XCTestCase {
    private var golden: [String: Any] { get throws { try LabNumbersTests.androidLab("sky-golden.json") } }

    private func list(_ key: String) throws -> [[String: Any]] { try XCTUnwrap(try golden[key] as? [[String: Any]], key) }
    private func num(_ v: Any?) -> Double { (v as? NSNumber)?.doubleValue ?? .nan }
    private func nums(_ v: Any?) -> [Double] { (v as? [Any])?.map(num) ?? [] }
    private func rows(_ v: Any?) -> [Double] { (v as? [Any])?.flatMap(nums) ?? [] }

    private func close(_ want: Double, _ got: Double, _ what: String, tol: Double = 1e-9, file: StaticString = #filePath, line: UInt = #line) {
        if abs(want - got) > tol * max(1, abs(want)) { XCTFail("\(what): want \(want), got \(got)", file: file, line: line) }
    }

    private func closeAll(_ want: [Double], _ got: [Double], _ what: String, tol: Double = 1e-9, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(want.count, got.count, "\(what): length", file: file, line: line)
        for i in 0..<min(want.count, got.count) { close(want[i], got[i], "\(what)[\(i)]", tol: tol, file: file, line: line) }
    }

    private func date(_ t: Any?) -> Date { Date(timeIntervalSince1970: num(t)) }

    // decision: PHN-450
    func testSunPositionMatchesTheLab() throws {
        let cases = try list("sun")
        XCTAssertFalse(cases.isEmpty)
        for c in cases {
            let s = SunPosition(date: date(c["t"]), latitude: num(c["lat"]), longitude: num(c["lon"]))
            let what = "\(c["place"] ?? "") at \(num(c["t"]))"
            close(num(c["elevation"]), s.elevation, "\(what) elevation", tol: 1e-8)
            close(num(c["azimuth"]), s.azimuth, "\(what) azimuth", tol: 1e-8)
            close(num(c["across"]), SkyView.across(azimuth: s.azimuth, latitude: num(c["lat"])), "\(what) across", tol: 1e-8)
        }
    }

    // decision: PHN-450
    func testSunriseAndSunsetMatchTheLab() throws {
        for c in try list("sunDays") {
            var cal = Calendar(identifier: .gregorian)
            cal.timeZone = try XCTUnwrap(TimeZone(identifier: c["zone"] as? String ?? ""))
            let d = SunDay(around: date(c["t"]), latitude: num(c["lat"]), longitude: num(c["lon"]), calendar: cal)
            let what = "\(c["place"] ?? "") around \(num(c["t"]))"
            if c["sunrise"] is NSNull { XCTAssertNil(d.sunrise, what) } else { XCTAssertEqual(d.sunrise?.timeIntervalSince1970, num(c["sunrise"]), "\(what) sunrise") }
            if c["sunset"] is NSNull { XCTAssertNil(d.sunset, what) } else { XCTAssertEqual(d.sunset?.timeIntervalSince1970, num(c["sunset"]), "\(what) sunset") }
        }
    }

    // decision: PHN-450
    func testMoonPhaseMatchesTheLab() throws {
        for c in try list("moons") {
            let m = MoonPhase(date: date(c["t"]))
            close(num(c["age"]), m.age, "moon age at \(num(c["t"]))")
            close(num(c["lit"]), m.lit, "moon lit at \(num(c["t"]))")
            XCTAssertEqual(m.waxing, c["waxing"] as? Bool)
        }
    }

    func testTheLabsOwnSkiesMatch() throws {
        for c in try list("labSkies") {
            let sky = SkyLook(elevation: num(c["elevation"]), evening: c["evening"] as? Bool ?? false)
            let what = "lab sky \(num(c["elevation"])) \(c["evening"] ?? "")"
            closeAll(rows(c["colors"]), sky.colors.flatMap { [$0.l, $0.a, $0.b] }, "\(what) colours")
            closeAll(nums(c["glow"]), [sky.glowLab.l, sky.glowLab.a, sky.glowLab.b], "\(what) glow")
            closeAll(rows(c["rgbStops"]), sky.rgbStops.flatMap { [Double($0.0), $0.1.0, $0.1.1, $0.1.2] }, "\(what) stops")
        }
    }

    private func field(_ f: SkyField) -> [Double] { [f.x, f.y, f.rx, f.ry, f.colour.r, f.colour.g, f.colour.b, f.alpha] }

    // decision: PHN-450, PHN-416
    func testTheOwnersChosenSkyMatchesTheLab() throws {
        let cases = try list("chosen")
        XCTAssertFalse(cases.isEmpty)
        for c in cases {
            let look = ChosenSky.look(elevation: num(c["elevation"]), across: num(c["across"]))
            let what = "chosen sky e=\(num(c["elevation"])) across=\(num(c["across"]))"
            closeAll(rows(c["colors"]), look.colors.flatMap { [$0.l, $0.a, $0.b] }, "\(what) colours")
            closeAll(rows(c["fields"]), look.fields.flatMap(field), "\(what) fields")
            closeAll(nums(c["sunGlow"]), [look.sunGlow.l, look.sunGlow.a, look.sunGlow.b], "\(what) glow")
            close(num(c["sunGlowStrength"]), look.sunGlowStrength, "\(what) glow strength")
            close(num(c["lightTop"]), look.lightTop, "\(what) lightTop")
            close(num(c["lightLow"]), look.lightLow, "\(what) lightLow")
            let ink = SkyInk(look)
            XCTAssertEqual(ink.top == .black, c["inkTopBlack"] as? Bool, "\(what) top ink")
            XCTAssertEqual(ink.low == .black, c["inkLowBlack"] as? Bool, "\(what) low ink")
        }
    }

    func testAstrasClockScheduleMatches() throws {
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let day = utc.date(from: DateComponents(year: 2026, month: 11, day: 14))!
        for c in try list("astraAt") {
            let h = num(c["hour"])
            let p = FerbSkyPalettes.at(day.addingTimeInterval((h * 3600).rounded()), calendar: utc)
            let what = "Astra at \(h)"
            closeAll(rows(c["colours"]), p.colours.flatMap { [$0.r, $0.g, $0.b] }, "\(what) colours")
            closeAll(nums(c["haze"]), field(p.haze), "\(what) haze")
            closeAll(nums(c["glow"]), field(p.glow), "\(what) glow")
            closeAll(nums(c["shade"]), field(p.shade), "\(what) shade")
        }
    }

    // decision: PHN-412
    func testSeasonsAndTheMonsoonBoxMatch() throws {
        for c in try list("seasons") {
            let got = Season.at(latitude: num(c["lat"]), longitude: num(c["lon"]), month: Int(num(c["month"])))
            XCTAssertEqual(got.rawValue, c["season"] as? String, "season at \(num(c["lat"])), \(num(c["lon"])) in month \(num(c["month"]))")
        }
    }

    func testTheSeasonLayersNumbersMatch() throws {
        let layer = SeasonLayer(season: .autumn, night: false)
        let randoms = try XCTUnwrap(try golden["randoms"] as? [[NSNumber]])
        for r in randoms { close(r[2].doubleValue, layer.r(r[0].intValue, r[1].intValue), "r(\(r[0]), \(r[1]))", tol: 1e-6) }
    }

    /// No weather: the layer is the season's alone, as the lab draws it under a clear sky.
    // decision: PHN-412
    func testTheSeasonLayerDrawsOnlyTheSeasons() {
        XCTAssertTrue(SeasonLayer.hasLayer(.autumn, night: false))
        XCTAssertTrue(SeasonLayer.hasLayer(.spring, night: true))
        XCTAssertFalse(SeasonLayer.hasLayer(.summer, night: false))
        XCTAssertTrue(SeasonLayer.hasLayer(.monsoon, night: true))
        XCTAssertFalse(SeasonLayer.hasLayer(.winter, night: true))
        XCTAssertEqual(SkyMoment.lab(12.5).season, .autumn)
        XCTAssertEqual(SkyMoment.lab(12.5, place: .sydney).season, .spring)
    }
}
