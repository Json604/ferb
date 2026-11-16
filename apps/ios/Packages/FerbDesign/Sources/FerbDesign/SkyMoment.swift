import Foundation
import SwiftUI

// The sky's only inputs: a coarse place and the time (kb PHN-450). SkyPlace and its four places are
// MobileScreens.swift's (lab 23a0fde); the place from a time zone (PHN-420, PHN-459) and SkyMoment are not from the
// lab: they are Android's design sky/SkyPlace.kt, so both phones put the sun in the same place.

/// The sky's input: a place and the time there. In the app it is the phone's one coarse reading or its time zone's
/// place, never sent anywhere.
public struct SkyPlace: Hashable, Identifiable, Sendable {
    public let name: String
    public let latitude: Double
    public let longitude: Double
    public let zone: String
    public var id: String { name }

    public init(name: String, latitude: Double, longitude: Double, zone: String) {
        self.name = name
        self.latitude = latitude
        self.longitude = longitude
        self.zone = zone
    }

    public static let delhi = SkyPlace(name: "Delhi", latitude: 28.61, longitude: 77.21, zone: "Asia/Kolkata")
    public static let london = SkyPlace(name: "London", latitude: 51.51, longitude: -0.13, zone: "Europe/London")
    public static let sanFrancisco = SkyPlace(name: "San Francisco", latitude: 37.77, longitude: -122.42, zone: "America/Los_Angeles")
    public static let sydney = SkyPlace(name: "Sydney", latitude: -33.87, longitude: 151.21, zone: "Australia/Sydney")
    /// MobileScreens.swift SkyPlace.all: the lab's four places.
    public static let all = [delhi, london, sanFrancisco, sydney]

    public var timeZone: TimeZone { TimeZone(identifier: zone) ?? .current }

    public var calendar: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = timeZone
        return c
    }

    /// "h:mm a" in the place's zone, as the lab writes a time (en_US, as Android's Locale.US).
    public func clock(_ date: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = calendar.timeZone
        f.dateFormat = "h:mm a"
        return f.string(from: date)
    }

    /// The time zone's principal place from the tz database (zone-places.json, the same table as Android's
    /// ZonePlaces.kt), or its standard offset's meridian on the equator when the zone has none.
    public static func fromZone(_ zone: TimeZone = .current, at date: Date = Date()) -> SkyPlace {
        if let p = ZonePlaces.all[zone.identifier] {
            return SkyPlace(name: zone.identifier, latitude: p.latitude, longitude: p.longitude, zone: zone.identifier)
        }
        let standard = Double(zone.secondsFromGMT(for: date)) - zone.daylightSavingTimeOffset(for: date)
        return SkyPlace(name: zone.identifier, latitude: 0, longitude: standard / 3600 * 15, zone: zone.identifier)
    }
}

/// The tz database's places (Resources/zone-places.json, made by tools/zone_places.py).
public enum ZonePlaces {
    public struct Place: Equatable, Sendable { public let latitude: Double; public let longitude: Double }

    private struct File: Decodable {
        let tzdata: String
        let places: [String: [Double]]
    }

    private static let file: File? = {
        guard let url = Bundle.module.url(forResource: "zone-places", withExtension: "json"),
              let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(File.self, from: data)
    }()

    /// The tz database's version the table was made from.
    public static var tzdata: String { file?.tzdata ?? "" }

    public static let all: [String: Place] = (file?.places ?? [:]).compactMapValues { v in
        v.count == 2 ? Place(latitude: v[0], longitude: v[1]) : nil
    }
}

/// Weather.swift Season: the season at a place and date. South Asia gets its monsoon (June to September); the south of
/// the world has its seasons the other way round.
public enum Season: String, CaseIterable, Sendable {
    case spring, summer, monsoon, autumn, winter

    public static func at(latitude: Double, longitude: Double, month: Int) -> Season {
        if (5...35).contains(latitude), (60...100).contains(longitude), (6...9).contains(month) { return .monsoon }
        let m = latitude >= 0 ? month : (month + 5) % 12 + 1
        switch m {
        case 3...5: return .spring
        case 6...8: return .summer
        case 9...11: return .autumn
        default: return .winter
        }
    }

    public static func at(_ place: SkyPlace, _ date: Date) -> Season {
        at(latitude: place.latitude, longitude: place.longitude, month: place.calendar.component(.month, from: date))
    }
}

/// Everything the sky needs for one moment (Android's SkyMoment; the lab's SkyNow).
public struct SkyMoment: Equatable, Sendable {
    public let date: Date
    public let place: SkyPlace

    public init(date: Date, place: SkyPlace) {
        self.date = date
        self.place = place
    }

    public var sun: SunPosition { SunPosition(date: date, latitude: place.latitude, longitude: place.longitude) }
    /// Where the sun is across the sky: -1 far left (rising), 1 far right (setting).
    public var across: Double { SkyView.across(azimuth: sun.azimuth, latitude: place.latitude) }
    public var moon: MoonPhase { MoonPhase(date: date) }
    public var look: SkyFrameLook { SkyFrameLook(date: date, latitude: place.latitude, longitude: place.longitude) }
    public var ink: SkyInk { SkyInk(look) }
    public var season: Season { Season.at(place, date) }
    /// Words.swift Moment.night: the season layer's fireflies come out below -4 degrees.
    public var night: Bool { sun.elevation < -4 }
    public var sunDay: SunDay { SunDay(around: date, latitude: place.latitude, longitude: place.longitude, calendar: place.calendar) }

    /// A local clock time on the given day at the place (the lab's SkyPlace.date(hour:), on a fixed day).
    public static func at(_ place: SkyPlace, year: Int, month: Int, day: Int, hour: Double) -> SkyMoment {
        let start = place.calendar.date(from: DateComponents(year: year, month: month, day: day))!
        return SkyMoment(date: start.addingTimeInterval(hour * 3600), place: place)
    }

    /// The day the screenshots are drawn on (the lab's last commit, 2026-11-14), in Delhi unless told otherwise.
    public static func lab(_ hour: Double, place: SkyPlace = .delhi, year: Int = 2026, month: Int = 11, day: Int = 14) -> SkyMoment {
        at(place, year: year, month: month, day: day, hour: hour)
    }

    public static func == (a: SkyMoment, b: SkyMoment) -> Bool { a.date == b.date && a.place == b.place }
}

private struct SkyMomentKey: EnvironmentKey { static let defaultValue: SkyMoment? = nil }

public extension EnvironmentValues {
    /// The moment PhoneScreen draws; HomePage takes the lab's words for it when it is given none.
    var skyMoment: SkyMoment? {
        get { self[SkyMomentKey.self] }
        set { self[SkyMomentKey.self] = newValue }
    }
}

/// The live sky's clock: the current moment at a place, renewed once a minute (the lab's `.periodic(by: 60)`;
/// Android's rememberSkyNow).
public struct LiveSky<Content: View>: View {
    let place: SkyPlace
    let content: (SkyMoment) -> Content

    public init(place: SkyPlace, @ViewBuilder content: @escaping (SkyMoment) -> Content) {
        self.place = place
        self.content = content
    }

    public var body: some View {
        TimelineView(.everyMinute) { context in content(SkyMoment(date: context.date, place: place)) }
    }
}
