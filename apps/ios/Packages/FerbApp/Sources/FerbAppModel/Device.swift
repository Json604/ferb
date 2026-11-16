import FerbCore
import FerbMobile
import Foundation
import Observation

// The phone's own facts for FERB on the Mac (T-1214, SENSE-420; Android: model/Device.kt): what iOS last said, turned
// into the library's DeviceReport. Pure: the iOS side (the shell's sensors) only calls in here, so the dedupe, the
// battery's steps and the place's 1 km rule are tested here.

/// The phone's facts as last read (empty or nil until iOS said them). Never coordinates.
public struct DeviceFacts: Equatable, Sendable {
    public static let locationOn = "on"
    public static let locationOff = "off"
    public static let locationDenied = "denied"
    public static let locationUnavailable = "unavailable"

    public var name = ""
    public var model = ""
    public var os = ""
    public var timeZone = ""
    public var utcOffsetMinutes = 0
    /// "on", "off" (the owner's switch), "denied" (on, but iOS's permission is gone) or "unavailable".
    public var location = DeviceFacts.locationOff
    public var place: DevicePlace? = nil
    public var batteryPercent: Int? = nil
    public var charging: Bool? = nil
    public var powerSaver: Bool? = nil
    /// "wifi", "mobile", "ethernet", "other", "offline", or "" before the first callback.
    public var network = ""
    public var locale = ""
    public var languages: [String] = []
    public var screenOn: Bool? = nil
    public var screenLocked: Bool? = nil

    public init(name: String = "", model: String = "", os: String = "") {
        self.name = name
        self.model = model
        self.os = os
    }

    public func report() -> DeviceReport {
        DeviceReport(name: name, model: model, os: os, timeZone: timeZone, utcOffsetMinutes: Int32(utcOffsetMinutes), location: location,
                     place: location == Self.locationOn ? place : nil, batteryPercent: batteryPercent.map { UInt8(min(max($0, 0), 100)) },
                     charging: charging, powerSaver: powerSaver, network: network, locale: locale, languages: languages,
                     screenOn: screenOn, screenLocked: screenLocked)
    }
}

/// Keeps [facts] from iOS's events and hands each new report to the library ([start]): when it changed, when the
/// phone opens (a start, a new pairing) and when the link comes online. Nothing here runs on a timer: every change
/// comes from a callback, and the library sends only what this connection hasn't had (and again on a new one). The
/// battery counts only when its percent crosses a 5-point step or charging changes. Kb PHN-530, PHN-531.
@MainActor
@Observable
public final class DeviceReporter {
    public static let batteryStep = 5
    public private(set) var facts: DeviceFacts
    @ObservationIgnored private var follow: Follow<Sent>?

    public init(_ initial: DeviceFacts) {
        facts = initial
    }

    public func zone(_ id: String, offsetMinutes: Int) { facts.timeZone = id; facts.utcOffsetMinutes = offsetMinutes }

    public func battery(percent: Int, charging: Bool) {
        let step = percent / Self.batteryStep
        if facts.charging == charging && facts.batteryPercent.map({ $0 / Self.batteryStep }) == step { return }
        facts.batteryPercent = percent
        facts.charging = charging
    }

    public func powerSaver(_ on: Bool) { facts.powerSaver = on }
    public func network(_ kind: String) { facts.network = kind }
    public func locale(_ tag: String, languages: [String]) { facts.locale = tag; facts.languages = languages }
    public func screen(on: Bool, locked: Bool) { facts.screenOn = on; facts.screenLocked = locked }

    /// The owner's switch and iOS's answer (`state`); the city once the geocoder named one (kept while on).
    public func location(_ state: String, place: DevicePlace?) {
        facts.location = state
        facts.place = state == DeviceFacts.locationOn ? (place ?? facts.place) : nil
    }

    private struct Sent: Equatable {
        let report: DeviceReport
        let open: Bool
        let online: Bool
    }

    /// Sends the report each time it, the phone's state or the link changes (no polling), while the store is open.
    public func start(_ api: PhoneApi) {
        follow = Follow({ [weak self] () -> Sent? in
            guard let self else { return nil }
            return Sent(report: facts.report(), open: api.state.isOpen, online: api.link.map { $0.paired && $0.phase == .online } ?? false)
        }) { (s: Sent) in
            if s.open { Task { _ = await api.reportDevice(s.report) } }
        }
    }

    public func stop() {
        follow?.stop()
        follow = nil
    }
}

/// A coarse fix, rounded to 2 decimals (about a kilometre): the sky's place, in memory only.
public struct Coarse: Equatable, Sendable {
    public let latitude: Double
    public let longitude: Double

    public init(latitude: Double, longitude: Double) {
        self.latitude = latitude
        self.longitude = longitude
    }

    public static func of(latitude: Double, longitude: Double) -> Coarse {
        Coarse(latitude: (latitude * 100).rounded() / 100, longitude: (longitude * 100).rounded() / 100)
    }
}

/// The city from coarse fixes: the geocoder is asked only for the first fix and after a move of [moveM] or more from
/// the last place it named (so a phone that stays put asks it once), never on a timer. Kb PHN-532.
@MainActor
@Observable
public final class PlaceKeeper {
    public static let moveM = 1_000.0
    @ObservationIgnored private let geocode: (Double, Double) async -> DevicePlace?
    @ObservationIgnored private var named: (Double, Double)?
    public private(set) var place: DevicePlace?
    public private(set) var coarse: Coarse?

    public init(geocode: @escaping (Double, Double) async -> DevicePlace?) {
        self.geocode = geocode
    }

    /// A fix arrived: the sky's place moves; the city is looked up again only after a kilometre or more.
    public func fix(latitude: Double, longitude: Double) async -> DevicePlace? {
        coarse = .of(latitude: latitude, longitude: longitude)
        if let last = named, Self.metres(last.0, last.1, latitude, longitude) < Self.moveM { return place }
        named = (latitude, longitude)
        if let p = await geocode(latitude, longitude) { place = p }
        return place
    }

    /// The owner turned it off: nothing is kept.
    public func forget() {
        named = nil
        place = nil
        coarse = nil
    }

    /// The great-circle distance in metres (haversine).
    public nonisolated static func metres(_ lat1: Double, _ lon1: Double, _ lat2: Double, _ lon2: Double) -> Double {
        let r = 6_371_000.0
        let rad = Double.pi / 180
        let dLat = (lat2 - lat1) * rad
        let dLon = (lon2 - lon1) * rad
        let a = pow(sin(dLat / 2), 2) + cos(lat1 * rad) * cos(lat2 * rad) * pow(sin(dLon / 2), 2)
        return 2 * r * asin(sqrt(min(max(a, 0), 1)))
    }
}
