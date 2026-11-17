import FerbDesign
import SwiftUI

// The gallery (Android's :gallery GalleryActivity): every token, piece and lab screen from FerbDesign with invented
// data, at any of the lab's places and any time of day, for the owner to look at on a phone or the simulator. It asks
// for no permission: the place is picked here (or is the time zone's own, PHN-420), never read. Not from the lab: the
// lab's stage (its place and hour controls) is a Mac window.

/// The whole gallery. A screen fills the phone; a swipe from the left edge goes back to the index.
public struct GalleryView: View {
    private enum Page: Hashable { case index, screen(LabScreen), showcase(String), motion }

    /// The places the lab offers, and the phone's own time zone's place.
    private let places: [(String, SkyPlace)] = [("This time zone", SkyPlace.fromZone())] + SkyPlace.all.map { ($0.name, $0) }

    @State private var page = UserDefaults.standard.bool(forKey: "FERBMotionTour") ? Page.motion : Page.index
    @State private var placeIndex = 1
    /// Local hour at the place; nil follows the real clock.
    @State private var hour: Double?
    @State private var playing = false

    public init() {}

    public var body: some View {
        TimelineView(.periodic(from: .now, by: playing ? 1.0 / 30 : 60)) { context in
            let moment = moment(at: context.date)
            content(moment)
        }
        .onChange(of: playing) { _, on in if on, hour == nil { hour = localHour(Date()) } }
    }

    private var place: SkyPlace { places[placeIndex].1 }

    private func localHour(_ date: Date) -> Double {
        let c = place.calendar.dateComponents([.hour, .minute, .second], from: date)
        return Double(c.hour ?? 0) + Double(c.minute ?? 0) / 60 + Double(c.second ?? 0) / 3600
    }

    /// The lab's Play a day: the whole day in twenty seconds.
    private func moment(at date: Date) -> SkyMoment {
        guard var h = hour else { return SkyMoment(date: date, place: place) }
        if playing { h = (date.timeIntervalSinceReferenceDate * 1.2).truncatingRemainder(dividingBy: 24) }
        let today = place.calendar.dateComponents([.year, .month, .day], from: date)
        return SkyMoment.at(place, year: today.year ?? 2026, month: today.month ?? 1, day: today.day ?? 1, hour: h)
    }

    @ViewBuilder private func content(_ moment: SkyMoment) -> some View {
        switch page {
        case .index:
            index(moment)
        case .screen(let screen):
            GeometryReader { geo in
                LabScreenView(screen: screen, moment: moment,
                              insets: PhoneInsets(top: geo.safeAreaInsets.top, bottom: geo.safeAreaInsets.bottom), glide: !playing)
            }
            .overlay(alignment: .leading) { backEdge }
            .preferredColorScheme(moment.ink.topIsDark ? .light : .dark)
        case .motion:
            GeometryReader { geo in
                MotionDemo(moment: moment, insets: PhoneInsets(top: geo.safeAreaInsets.top, bottom: geo.safeAreaInsets.bottom))
            }
            .overlay(alignment: .leading) { backEdge }
            .preferredColorScheme(moment.ink.topIsDark ? .light : .dark)
        case .showcase(let name):
            ScrollView([.vertical, .horizontal]) {
                Showcases.all.first { $0.name == name }?.view()
            }
            .background(Ink.black)
            .overlay(alignment: .leading) { backEdge }
            .preferredColorScheme(.dark)
        }
    }

    /// A strip along the left edge: swiping right from it goes back.
    private var backEdge: some View {
        Color.clear.frame(width: 20).contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 20).onEnded { g in if g.translation.width > 60 { page = .index } })
            .accessibilityElement()
            .accessibilityLabel(Text("Back to the gallery"))
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { page = .index }
    }

    private func index(_ moment: SkyMoment) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Text("FERB design gallery").type(28, .semibold, Ink.white)
                Text("Slice I3: the glass lab's phone design in SwiftUI. Everything shown is invented. Swipe from the left edge to come back.")
                    .type(14, .regular, Ink.onDark).fixedSize(horizontal: false, vertical: true)
                Text("Place").type(15, .semibold, Ink.white).padding(.top, 10)
                chips(places.map(\.0), chosen: placeIndex) { placeIndex = $0 }
                Text("Time").type(15, .semibold, Ink.white)
                let times: [(String, Double?)] = [("Now", nil)] + LabSample.times.map { ($0.0, Optional($0.1)) }
                chips(times.map(\.0) + [playing ? "Stop the day" : "Play a day"],
                      chosen: playing ? -1 : (times.firstIndex { $0.1 == hour } ?? -1)) { i in
                    if i == times.count { playing.toggle() } else { playing = false; hour = times[i].1 }
                }
                Text(String(format: "%@ there; the sun at %.1f degrees, %@.", moment.place.clock(moment.date), moment.sun.elevation,
                            moment.season.rawValue))
                    .type(13, .regular, Ink.onDark)
                Text("Screens").type(15, .semibold, Ink.white).padding(.top, 10)
                Card {
                    ForEach(Array(LabScreen.allCases.enumerated()), id: \.offset) { i, s in
                        if i > 0 { Divide() }
                        CardRow(title: s.title, detail: s.note, onClick: { page = .screen(s) }) { EmptyView() } trailing: { Opens() }
                    }
                }
                Text("Motion").type(15, .semibold, Ink.white).padding(.top, 10)
                Card {
                    CardRow(title: "Tabs and sheets", detail: "The app's motion (kb PHN-620..624) over the lab's pages: tap the tabs, the rows, the Mac",
                            onClick: { page = .motion }) { EmptyView() } trailing: { Opens() }
                }
                Text("Tokens and components").type(15, .semibold, Ink.white).padding(.top, 10)
                Card {
                    ForEach(Array(Showcases.all.enumerated()), id: \.offset) { i, s in
                        if i > 0 { Divide() }
                        CardRow(title: s.name, onClick: { page = .showcase(s.name) }) { EmptyView() } trailing: { Opens() }
                    }
                }
            }
            .padding(16)
        }
        .background(Ink.black)
        .preferredColorScheme(.dark)
    }

    private func chips(_ labels: [String], chosen: Int, pick: @escaping (Int) -> Void) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(labels.enumerated()), id: \.offset) { i, label in
                    let on = i == chosen
                    Text(label).type(14, .medium, on ? Ink.black : Ink.white).lineLimit(1)
                        .padding(.horizontal, 14).padding(.vertical, 10)
                        .background(on ? Ink.white : Ink.darkFill, in: Capsule())
                        .pressable { pick(i) }
                }
            }
        }
    }
}
