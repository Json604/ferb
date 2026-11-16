import CoreLocation
import CoreWLAN
import Foundation
import NotchCore

/// One location reading: where this Mac is, and to within how many meters.
struct LocationFix: Equatable {
    let latitude: Double
    let longitude: Double
    let accuracy: Double
}

/// A city-level place, reverse-geocoded from a fix (Apple's geocoder: the fix goes to Apple to be named).
struct DevicePlace: Equatable {
    let city: String
    let region: String
    let country: String
    let countryCode: String

    var report: [String: Any] { ["city": city, "region": region, "country": country, "country_code": countryCode] }
}

/// The Location permission and the readings FERB.app makes with it (the CLLocationManager seam; tests fake it).
@MainActor
protocol LocationSource: AnyObject {
    var status: GrantStatus { get }
    /// macOS's own question; the status once the owner answered it.
    func requestAuthorization() async -> GrantStatus
    /// A rough fix (about a kilometer), for the city; nil when none comes.
    func coarseFix() async -> LocationFix?
    /// One precise fix, for a task the owner said Yes to; nil when none comes.
    func preciseFix() async -> LocationFix?
    func place(of fix: LocationFix) async -> DevicePlace?
    /// The Wi-Fi network's name (macOS gives it only to an app with the Location permission).
    func wifiName() -> String?
    /// Called when the permission changes or the Mac moved a significant distance.
    var onChange: (() -> Void)? { get set }
}

/// FERB.app tells the daemon this Mac's Location permission, city and Wi-Fi name (`device.report`, FERB.app's own),
/// at launch, on each connection and on every change; and reads one precise fix when a task's owner said Yes
/// (`device.location_wanted`, GATE-613). Coordinates never leave FERB.app otherwise: the city is what the daemon keeps.
@MainActor
final class DeviceLocationReporter {
    let rpc: DaemonRPCTransport
    let source: LocationSource
    /// The fix last named, and its place: a new name is asked of Apple's geocoder only after a move of `renameAfter`.
    private var named: (fix: LocationFix, place: DevicePlace)?
    var renameAfter: CLLocationDistance = 1_000

    init(rpc: DaemonRPCTransport, source: LocationSource) {
        self.rpc = rpc
        self.source = source
        source.onChange = { [weak self] in Task { await self?.report() } }
    }

    /// The report's parameters: the permission as macOS says it, and the place and Wi-Fi name only while it is on.
    static func reportParams(status: GrantStatus, place: DevicePlace?, wifi: String?) -> [String: Any] {
        var params: [String: Any] = [:]
        switch status {
        case .granted, .denied, .notDetermined: params["location"] = status.rawValue
        case .targetNotRunning, .unknown: break
        }
        let on = status == .granted
        params["place"] = on ? (place?.report ?? NSNull()) : NSNull()
        params["wifi"] = on ? (wifi ?? NSNull()) : NSNull()
        return params
    }

    /// The place for a fix: the one already named when the Mac hasn't moved far, else Apple's geocoder.
    func placeFor(_ fix: LocationFix) async -> DevicePlace? {
        if let named, Self.distance(named.fix, fix) < renameAfter { return named.place }
        guard let place = await source.place(of: fix) else { return named?.place }
        named = (fix, place)
        return place
    }

    func report() async {
        let status = source.status
        var place: DevicePlace?
        var wifi: String?
        if status == .granted {
            if let fix = await source.coarseFix() { place = await placeFor(fix) }
            wifi = source.wifiName()
        }
        _ = try? await rpc.call("device.report", params: Self.reportParams(status: status, place: place, wifi: wifi))
    }

    /// One precise fix for the daemon's request, or why there is none. Only on its request, after the owner's Yes.
    func answer(requestID: String) async {
        var params: [String: Any] = ["request_id": requestID]
        if source.status != .granted {
            params["error"] = "FERB.app doesn't have the Location permission"
        } else if let fix = await source.preciseFix() {
            params["exact"] = ["latitude": fix.latitude, "longitude": fix.longitude, "accuracy_m": fix.accuracy]
        } else {
            params["error"] = "macOS gave no location in time"
        }
        _ = try? await rpc.call("device.report", params: params)
    }

    static func distance(_ a: LocationFix, _ b: LocationFix) -> CLLocationDistance {
        CLLocation(latitude: a.latitude, longitude: a.longitude)
            .distance(from: CLLocation(latitude: b.latitude, longitude: b.longitude))
    }
}

/// The real Location permission and readings (CoreLocation, CLGeocoder, CoreWLAN): one per app, on the main thread.
@MainActor
final class SystemLocationSource: NSObject, LocationSource, CLLocationManagerDelegate {
    static let shared = SystemLocationSource()

    private let manager = CLLocationManager()
    private var authorizationWaiters: [CheckedContinuation<GrantStatus, Never>] = []
    private var fixWaiters: [(id: Int, precise: Bool, resume: CheckedContinuation<LocationFix?, Never>)] = []
    private var nextWaiter = 0
    private var watching = false
    /// How long a fix may take before the reading gives up (a technical bound).
    var fixWait: Duration = .seconds(20)
    var onChange: (() -> Void)?

    override init() {
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyKilometer
        watchIfAllowed()
    }

    var status: GrantStatus { Self.grant(manager.authorizationStatus) }

    static func grant(_ status: CLAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorizedAlways, .authorizedWhenInUse: return .granted
        case .denied, .restricted: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }

    func requestAuthorization() async -> GrantStatus {
        guard status == .notDetermined else { return status }
        return await withCheckedContinuation { continuation in
            authorizationWaiters.append(continuation)
            manager.requestWhenInUseAuthorization()
        }
    }

    func coarseFix() async -> LocationFix? {
        if let last = manager.location, -last.timestamp.timeIntervalSinceNow < 600 { return Self.fix(last) }
        return await fix(precise: false)
    }

    func preciseFix() async -> LocationFix? {
        await fix(precise: true)
    }

    private func fix(precise: Bool) async -> LocationFix? {
        guard status == .granted else { return nil }
        let wait = fixWait
        nextWaiter += 1
        let id = nextWaiter
        return await withCheckedContinuation { continuation in
            fixWaiters.append((id, precise, continuation))
            if precise { manager.desiredAccuracy = kCLLocationAccuracyBest }
            manager.requestLocation()
            Task { [weak self] in
                try? await Task.sleep(for: wait)
                self?.giveUp(id)
            }
        }
    }

    /// A fix to the waiting readings: a rough one ends the rough readings, a precise one (100 m or better) every one;
    /// `all` ends them all (an error).
    private func resolveFixes(with fix: LocationFix?, all: Bool) {
        let precise = fix.map { $0.accuracy >= 0 && $0.accuracy <= 100 } ?? false
        let ready = fixWaiters.filter { all || !$0.precise || precise }
        fixWaiters.removeAll { waiter in ready.contains { $0.id == waiter.id } }
        for waiter in ready { waiter.resume.resume(returning: fix) }
        if !fixWaiters.contains(where: \.precise) { manager.desiredAccuracy = kCLLocationAccuracyKilometer }
    }

    /// A reading past `fixWait`: the fix macOS has from the last two minutes (its accuracy said as it is), else none.
    private func giveUp(_ id: Int) {
        guard let at = fixWaiters.firstIndex(where: { $0.id == id }) else { return }
        let waiter = fixWaiters.remove(at: at)
        let recent = manager.location.flatMap { -$0.timestamp.timeIntervalSinceNow < 120 ? Self.fix($0) : nil }
        waiter.resume.resume(returning: recent)
        if !fixWaiters.contains(where: \.precise) { manager.desiredAccuracy = kCLLocationAccuracyKilometer }
    }

    func place(of fix: LocationFix) async -> DevicePlace? {
        let location = CLLocation(latitude: fix.latitude, longitude: fix.longitude)
        guard let mark = try? await CLGeocoder().reverseGeocodeLocation(location).first else { return nil }
        return DevicePlace(city: mark.locality ?? "", region: mark.administrativeArea ?? "",
                           country: mark.country ?? "", countryCode: mark.isoCountryCode ?? "")
    }

    func wifiName() -> String? {
        guard status == .granted, let name = CWWiFiClient.shared().interface()?.ssid(), !name.isEmpty else { return nil }
        return name
    }

    private func watchIfAllowed() {
        guard status == .granted, !watching else { return }
        watching = true
        manager.startMonitoringSignificantLocationChanges()
    }

    static func fix(_ location: CLLocation) -> LocationFix {
        LocationFix(latitude: location.coordinate.latitude, longitude: location.coordinate.longitude,
                    accuracy: location.horizontalAccuracy)
    }

    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        MainActor.assumeIsolated {
            let now = status
            if now != .notDetermined {
                let waiting = authorizationWaiters
                authorizationWaiters = []
                for waiter in waiting { waiter.resume(returning: now) }
            }
            watchIfAllowed()
            onChange?()
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        MainActor.assumeIsolated {
            guard let last = locations.last else { return }
            let hadWaiters = !fixWaiters.isEmpty
            resolveFixes(with: Self.fix(last), all: false)
            if !hadWaiters { onChange?() }                  // a significant move: the city may have changed
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        MainActor.assumeIsolated { resolveFixes(with: nil, all: true) }
    }
}
