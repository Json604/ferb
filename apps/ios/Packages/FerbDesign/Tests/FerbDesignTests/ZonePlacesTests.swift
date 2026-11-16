import Foundation
import XCTest
@testable import FerbDesign

final class ZonePlacesTests: XCTestCase {
    /// Android's ZonePlaces.kt table, read from its source.
    private func androidTable() throws -> (version: String, places: [String: (Double, Double)]) {
        let url = LabNumbersTests.repo.appendingPathComponent("apps/android/design/src/main/kotlin/com/ferb/design/sky/ZonePlaces.kt")
        let text = try String(contentsOf: url, encoding: .utf8)
        let version = try XCTUnwrap(text.range(of: #"TZDATA = "([^"]+)""#, options: .regularExpression).map { String(text[$0]) })
            .replacingOccurrences(of: "TZDATA = ", with: "").replacingOccurrences(of: "\"", with: "")
        let start = try XCTUnwrap(text.range(of: "TABLE: String ="))
        let end = try XCTUnwrap(text.range(of: "val all", range: start.upperBound..<text.endIndex))
        let body = text[start.upperBound..<end.lowerBound]
        let regex = try NSRegularExpression(pattern: "\"([^\"]*)\"")
        let ns = String(body)
        let table = regex.matches(in: ns, range: NSRange(ns.startIndex..., in: ns)).map { String(ns[Range($0.range(at: 1), in: ns)!]) }.joined()
        var places: [String: (Double, Double)] = [:]
        for entry in table.split(separator: ";") {
            let p = entry.split(separator: "|")
            places[String(p[0])] = (Double(p[1])!, Double(p[2])!)
        }
        return (version, places)
    }

    /// The iPhone's table and Android's are the same, entry for entry, from the same tz database.
    // decision: PHN-459, PHN-420
    func testTheZoneTableIsAndroidsEntryForEntry() throws {
        let android = try androidTable()
        XCTAssertEqual(ZonePlaces.tzdata, android.version)
        XCTAssertGreaterThan(android.places.count, 400)
        XCTAssertEqual(Set(ZonePlaces.all.keys), Set(android.places.keys))
        for (zone, (lat, lon)) in android.places {
            let ours = try XCTUnwrap(ZonePlaces.all[zone], zone)
            XCTAssertEqual(ours.latitude, lat, accuracy: 1e-9, zone)
            XCTAssertEqual(ours.longitude, lon, accuracy: 1e-9, zone)
        }
    }

    /// Without a location the sky uses the time zone's principal place, old names included (Android's
    /// PlaceAndMotionTest.aTimeZoneGivesACoarsePlace).
    // decision: PHN-420, PHN-459
    func testATimeZoneGivesACoarsePlace() throws {
        let kolkata = SkyPlace.fromZone(try XCTUnwrap(TimeZone(identifier: "Asia/Kolkata")))
        XCTAssertEqual(kolkata.latitude, 22.53, accuracy: 0.01)
        XCTAssertEqual(kolkata.longitude, 88.37, accuracy: 0.01)
        XCTAssertEqual(SkyPlace.fromZone(try XCTUnwrap(TimeZone(identifier: "Asia/Calcutta"))).latitude, kolkata.latitude)
        let sydney = SkyPlace.fromZone(try XCTUnwrap(TimeZone(identifier: "Australia/Sydney")))
        XCTAssertTrue(sydney.latitude < -33 && sydney.longitude > 151)
        // A zone with no place (a fixed offset) falls back to its meridian on the equator.
        let fixed = SkyPlace.fromZone(try XCTUnwrap(TimeZone(identifier: "Etc/GMT-5")))
        XCTAssertEqual(fixed.latitude, 0)
        XCTAssertEqual(fixed.longitude, 75)
    }
}
