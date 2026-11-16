import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB.app's Location hand-off (SENSE-420): the city and Wi-Fi name told to the daemon only while the permission is
/// on, Apple's geocoder asked again only after a real move, and a precise fix only on the daemon's request.
@MainActor
final class DeviceLocationTests: XCTestCase {
    /// The CLLocationManager seam: what macOS would answer.
    @MainActor final class FakeSource: LocationSource {
        var status: GrantStatus = .granted
        var coarse: LocationFix? = LocationFix(latitude: 18.5204, longitude: 73.8567, accuracy: 1_000)
        var precise: LocationFix? = LocationFix(latitude: 18.52043, longitude: 73.85674, accuracy: 8)
        var wifi: String? = "Home"
        var named: [LocationFix] = []
        var preciseReads = 0
        var onChange: (() -> Void)?

        func requestAuthorization() async -> GrantStatus { status }
        func coarseFix() async -> LocationFix? { coarse }
        func preciseFix() async -> LocationFix? { preciseReads += 1; return precise }
        func place(of fix: LocationFix) async -> DevicePlace? {
            named.append(fix)
            return DevicePlace(city: "Pune", region: "Maharashtra", country: "India", countryCode: "IN")
        }
        func wifiName() -> String? { wifi }
    }

    private func last(_ rpc: ScriptedRPC) -> [String: Any] {
        rpc.calls.last { $0.0 == "device.report" }?.1 ?? [:]
    }

    // decision: SENSE-421
    func testTheCityAndWifiGoToTheDaemonOnlyWhileLocationIsOn() async {
        let (rpc, source) = (ScriptedRPC(), FakeSource())
        let reporter = DeviceLocationReporter(rpc: rpc, source: source)
        await reporter.report()
        let on = last(rpc)
        XCTAssertEqual(on["location"] as? String, "granted")
        XCTAssertEqual((on["place"] as? [String: Any])?["city"] as? String, "Pune")
        XCTAssertEqual(on["wifi"] as? String, "Home")
        XCTAssertNil(on["latitude"])                                          // never coordinates in a report
        XCTAssertFalse("\(on)".contains("18.52"))
        source.status = .denied
        await reporter.report()
        let off = last(rpc)
        XCTAssertEqual(off["location"] as? String, "denied")
        XCTAssertTrue(off["place"] is NSNull && off["wifi"] is NSNull)
    }

    // decision: SENSE-421
    func testApplesGeocoderIsAskedAgainOnlyAfterAMove() async {
        let (rpc, source) = (ScriptedRPC(), FakeSource())
        let reporter = DeviceLocationReporter(rpc: rpc, source: source)
        await reporter.report()
        source.coarse = LocationFix(latitude: 18.5210, longitude: 73.8570, accuracy: 1_000)   // about 70 m away
        await reporter.report()
        XCTAssertEqual(source.named.count, 1)
        source.coarse = LocationFix(latitude: 19.0760, longitude: 72.8777, accuracy: 1_000)   // another city
        await reporter.report()
        XCTAssertEqual(source.named.count, 2)
    }

    // decision: GATE-613
    func testAPreciseFixIsReadOnlyOnTheDaemonsRequest() async {
        let (rpc, source) = (ScriptedRPC(), FakeSource())
        let reporter = DeviceLocationReporter(rpc: rpc, source: source)
        await reporter.report()
        XCTAssertEqual(source.preciseReads, 0)
        await reporter.answer(requestID: "q1")
        let answer = last(rpc)
        XCTAssertEqual(answer["request_id"] as? String, "q1")
        let exact = answer["exact"] as? [String: Any]
        XCTAssertEqual(exact?["latitude"] as? Double, 18.52043)
        XCTAssertEqual(exact?["accuracy_m"] as? Double, 8)
        source.status = .denied
        await reporter.answer(requestID: "q2")
        XCTAssertNil(last(rpc)["exact"])
        XCTAssertNotNil(last(rpc)["error"])
        XCTAssertEqual(source.preciseReads, 1)
    }
}
