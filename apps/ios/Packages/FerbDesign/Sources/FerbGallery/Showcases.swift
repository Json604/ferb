import FerbDesign
import SwiftUI

// The gallery's showcases: every token and piece of FerbDesign with invented data, the same set as Android's
// design preview/Showcase.kt so the two galleries can be laid side by side. Not from the lab: the lab shows its
// pieces only inside its screens and its Mac design book.

/// One showcase: a name and its view.
public struct Showcase: Identifiable {
    public let name: String
    public let view: () -> AnyView
    public var id: String { name }
}

public enum Showcases {
    public static let all: [Showcase] = [
        Showcase(name: "tokens") { AnyView(ShowTokens()) },
        Showcase(name: "topbar") { AnyView(OnSky(moment: .lab(12.5)) { ForEach(PhoneTab.allCases, id: \.self) { TopBar(chosen: $0).padding(.horizontal, -16) } }) },
        Showcase(name: "marks-pills") { AnyView(ShowMarksAndPills()) },
        Showcase(name: "rows") { AnyView(ShowRows()) },
        Showcase(name: "conversation") { AnyView(ShowConversation()) },
        Showcase(name: "sheet-pieces") { AnyView(ShowSheetPieces()) },
        Showcase(name: "keypad") { AnyView(SecretKeypad().frame(width: 412)) },
        Showcase(name: "qr-frame") { AnyView(OnSky(moment: .lab(12.5)) { QrFrame(hint: "Point your phone at the code on your Mac.") }) },
        Showcase(name: "link-pictures") { AnyView(OnSky(moment: .lab(17.4)) { ForEach([LinkKind.direct, .relay, .search], id: \.self) { LinkPicture(kind: $0).padding(.bottom, 8) } }) },
        Showcase(name: "marks-files") { AnyView(ShowMarksAndFiles()) },
        Showcase(name: "glass") { AnyView(ShowGlass()) },
        Showcase(name: "skies-delhi") { AnyView(ShowSkies(place: .delhi)) },
        Showcase(name: "skies-london-june") { AnyView(ShowSkies(place: .london, month: 6, day: 21)) },
        Showcase(name: "skies-sydney") { AnyView(ShowSkies(place: .sydney)) },
        Showcase(name: "seasons") { AnyView(ShowSeasons()) },
        Showcase(name: "astra-presets") { AnyView(ShowAstraPresets()) },
        Showcase(name: "moons") { AnyView(ShowMoons()) },
    ]
}

/// A piece of sky to show a component where it lives: the sky and its ink.
struct OnSky<Content: View>: View {
    let moment: SkyMoment
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 12) { content }
            .padding(16)
            .frame(width: 412)
            .background { SkyView(date: moment.date, latitude: moment.place.latitude, longitude: moment.place.longitude) }
            .environment(\.skyInk, moment.ink)
    }
}

struct ShowMarksAndPills: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                ForEach([MarkKind.done, .asking, .failed, .running, .waiting], id: \.self) { Mark(kind: $0) }
                Mark(kind: .running, size: 20)
            }
            Pill(title: "Unlock with Face ID", symbol: "faceid")
            Pill(title: "Not now", kind: .grey)
            Pill(title: "Cancel", kind: .plain, height: 44)
            HStack(spacing: 8) {
                Pill(title: "Watch live", kind: .grey, symbol: "eye", height: 46)
                Pill(title: "Stop", kind: .black, symbol: "stop.fill", height: 46)
            }
            HStack(spacing: 10) {
                SheetIcon(symbol: "terminal")
                SheetIcon(symbol: "key", asking: false)
                CloseButton()
                PhoneSwitch(on: true)
                PhoneSwitch(on: false)
            }
            BiometricNote()
        }
        .padding(16)
        .frame(width: 412)
        .background(Ink.white)
    }
}

struct ShowRows: View {
    var body: some View {
        OnSky(moment: .lab(12.5)) {
            SkyLabel(text: "Waiting for you")
            Card {
                CardRow(title: "Renew the domain", detail: "Asks before paying $12.99") { Mark(kind: .asking) } trailing: { Opens() }
                Divide()
                CardRow(title: "Compare standing desks", detail: "Comparing price and delivery") { Mark(kind: .running) } trailing: { Opens(value: "2 of 3") }
                Divide()
                CardRow(title: "Thesis draft v7.docx", detail: "2.4 MB, edited today") { FileIcon(name: "Thesis draft v7.docx", width: 26) } trailing: { Opens() }
                Divide()
                CardRow(title: "Stay connected", detail: "Questions arrive while the app is closed") { RowIcon(symbol: "bell") } trailing: { PhoneSwitch(on: true) }
                Divide()
                CardRow(title: "Forget this Mac", danger: true) { RowIcon(symbol: "link", danger: true) }
            }
        }
    }
}

struct ShowConversation: View {
    var body: some View {
        OnSky(moment: .lab(12.5)) {
            OwnerBubble("What's left on my calendar today?")
            FerbBubble("Two things: the design review at 4, and a call with the bank at 6:30.")
            TypingDots()
            if case let .task(headline, progress, parts) = LabSample.chat[3] {
                TaskCard(headline: headline, progress: progress, parts: parts)
            }
            PhoneComposer(placeholder: "Add to this")
            PhoneComposer(typed: "Compare the three standing desks")
        }
    }
}

struct ShowSheetPieces: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ExactCommand(LabSample.command)
            ExactItem(title: "Northfield Desk Pro", detail: "Delivers Tuesday", value: "$379")
            SecretCodeBoxes(filled: 4)
            SecretCodeBoxes(filled: 0)
            PairingCode(LabSample.code)
            PairStep(1, "On your Mac, open FERB and choose Pair a phone.")
            HStack {
                PairAction(symbol: "keyboard", title: "Type code")
                PairAction(symbol: "laptopcomputer", title: "Get the Mac app")
                PairAction(symbol: "questionmark.circle", title: "Help")
            }
        }
        .padding(16)
        .frame(width: 412)
        .background(Ink.white)
    }
}

struct ShowMarksAndFiles: View {
    private let names = ["Lease agreement.pdf", "Thesis draft v7.docx", "Budget.xlsx", "Pitch.key", "Beach.heic", "Clip.mov", "Memo.m4a",
                         "main.rs", "Backup.zip", "notes.md", "README"]
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 16) {
                FerbMark(size: 84)
                FerbMark(size: 42)
                LampDot(color: Lamp.amber, size: 8)
                LampDot(color: Lamp.green, size: 8)
                LampDot(color: Lamp.red, size: 8)
                LampDot(color: Tone.text, size: 8)
            }
            LazyVGrid(columns: Array(repeating: GridItem(.fixed(40), spacing: 14), count: 7), alignment: .leading, spacing: 14) {
                ForEach(names, id: \.self) { FileIcon(name: $0, width: 34).padding(.leading, 6) }
            }
        }
        .padding(16)
        .frame(width: 412)
        .background(Tone.ink1)
    }
}

/// The glass depths over the sky (Theme.swift GlassBackground): a panel with a card, a control, an inset and chips.
struct ShowGlass: View {
    var body: some View {
        OnSky(moment: .lab(17.4)) {
            VStack(alignment: .leading, spacing: 10) {
                Text("A panel over the sky").ink(17, .semibold)
                Text("A card inside it: fill1").ink(13, .medium).frame(maxWidth: .infinity, alignment: .leading).padding(14).glassCard(Radius.card)
                Text("A control: fill2").ink(12.5, .medium).frame(maxWidth: .infinity, alignment: .leading).padding(10)
                    .glass(RoundedRectangle(cornerRadius: Radius.button, style: .continuous), .control)
                Text("An inset: the well").ink(12.5, .regular, .dim).frame(maxWidth: .infinity, alignment: .leading).padding(10)
                    .glass(RoundedRectangle(cornerRadius: Radius.list, style: .continuous), .inset)
                HStack(spacing: 6) {
                    Chip(text: "Asking", lamp: Lamp.amber)
                    Chip(text: "Done", lamp: Lamp.green)
                    Chip(text: "Couldn't do it", lamp: Lamp.red)
                    Chip(text: "Working", lamp: Tone.text)
                }
            }
            .padding(18)
            .glass(RoundedRectangle(cornerRadius: Radius.window, style: .continuous), .panel)
            .environment(\.colorScheme, .dark)
            Spacer(minLength: 0)
        }
        .frame(height: 420)
    }
}

/// The day's skies at the gallery's eight times, at one place.
struct ShowSkies: View {
    let place: SkyPlace
    var month = 11
    var day = 14
    var body: some View {
        HStack(spacing: 4) {
            ForEach(LabSample.times, id: \.0) { name, hour in
                let m = SkyMoment.lab(hour, place: place, month: month, day: day)
                VStack(spacing: 0) {
                    SkyView(date: m.date, latitude: place.latitude, longitude: place.longitude).frame(width: 100, height: 222)
                    Text(name).font(.system(size: 10)).foregroundStyle(.white)
                }
            }
        }
        .padding(8)
        .background(Color.black)
    }
}

/// Astra's eight presets as its own renderer draws them, laid out as Android's showcase lays them out.
struct ShowAstraPresets: View {
    var body: some View {
        let phases = FerbSkyPhase.allCases.filter { $0 != .now }
        ZStack(alignment: .topLeading) {
            Color(hex: 0x11141B)
            ForEach(Array(phases.enumerated()), id: \.offset) { i, phase in
                FerbSkyBackground(phase: phase).frame(width: 280, height: 175)
                    .offset(x: CGFloat(20 + 292 * (i % 4)), y: CGFloat(20 + 218 * (i / 4)))
            }
        }
        .frame(width: 1196, height: 464)
    }
}

/// The season's layer over the sky: autumn leaves (Delhi, November), spring petals (Sydney), fireflies (summer and
/// monsoon nights).
struct ShowSeasons: View {
    var body: some View {
        let cases: [(SkyMoment, Season, Bool)] = [
            (.lab(9.0), .autumn, false), (.lab(9.0, place: .sydney), .spring, false),
            (.lab(22.5, place: .london, month: 7, day: 14), .summer, true), (.lab(22.5, month: 8, day: 14), .monsoon, true),
        ]
        HStack(spacing: 4) {
            ForEach(Array(cases.enumerated()), id: \.offset) { _, c in
                ZStack {
                    SkyView(date: c.0.date, latitude: c.0.place.latitude, longitude: c.0.place.longitude)
                    SeasonLayer(season: c.1, night: c.2)
                }
                .frame(width: 200, height: 440)
            }
        }
        .padding(4)
        .background(Color.black)
        .stillMotion()
    }
}

/// Tonight's moon on four nights of one cycle (the moon is drawn at 0.8 across, 0.17 down).
struct ShowMoons: View {
    var body: some View {
        HStack(spacing: 4) {
            ForEach([10, 17, 24, 30], id: \.self) { day in
                let m = SkyMoment.lab(22.5, day: day)
                SkyView(date: m.date, latitude: m.place.latitude, longitude: m.place.longitude).frame(width: 150, height: 300)
            }
        }
        .padding(4)
        .background(Color.black)
    }
}

/// The tokens: the phone's ink, the warm ramp and lamps, the type sizes, radii and spacing.
struct ShowTokens: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Ink (the phone's colours)").type(15, .semibold)
            swatches([("black", Ink.black), ("white", Ink.white), ("detail", Ink.detail), ("faint", Ink.faint), ("fill", Ink.fill),
                      ("line", Ink.line), ("darkFill", Ink.darkFill), ("onDark", Ink.onDark), ("amber", Ink.amber), ("red", Ink.red)])
            Text("Tone and Lamp (the glass pieces)").type(15, .semibold)
            swatches([("ink0", Tone.ink0), ("ink1", Tone.ink1), ("ink2", Tone.ink2), ("ink3", Tone.ink3), ("ink4", Tone.ink4),
                      ("line", Tone.line), ("textFaint", Tone.textFaint), ("textDim", Tone.textDim), ("text", Tone.text),
                      ("amber", Lamp.amber), ("green", Lamp.green), ("red", Lamp.red), ("ring", LabColors.waitingRing), ("keypad", LabColors.keypad)])
            Text("Type (system face)").type(15, .semibold)
            ForEach([(42.0, "Greeting 42 medium"), (28, "Locked 28 semibold"), (24, "Page title 24 semibold"), (22, "Sheet title 22 semibold"),
                     (16.5, "Row title 16.5 semibold"), (16, "Body 16"), (15.5, "Explain 15.5"), (15, "Label 15 semibold"), (14, "Detail 14")], id: \.1) { size, name in
                Text(name).type(size, size >= 22 || size == 16.5 || size == 15 ? .semibold : .regular).lineLimit(1)
            }
            Text("Radii").type(15, .semibold)
            LazyVGrid(columns: Array(repeating: GridItem(.fixed(64), spacing: 8), count: 5), alignment: .leading, spacing: 8) {
                ForEach([("card", 28.0), ("sheet", 44), ("task", 26), ("code", 24), ("bubble", 22), ("part", 18), ("pill", 16), ("icon", 15), ("box", 14)], id: \.0) { name, r in
                    VStack(spacing: 2) {
                        RoundedRectangle(cornerRadius: r, style: .continuous).fill(Ink.fill).frame(width: 64, height: 48)
                        Text("\(name) \(r, specifier: "%g")").type(10, .regular, Ink.detail).lineLimit(1)
                    }
                }
            }
            Text("Space \(Space.all.map { "\(Int($0))" }.joined(separator: ", "))").type(14, .regular, Ink.detail)
            Text("Motion: press 0.97 on a 0.2 s spring (bounce 0.15); sheets 0.45 s, bounce 0.12; the sky glides 2.2 s; the season fades 1.4 s.")
                .type(13, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(width: 412)
        .background(Ink.white)
    }

    private func swatches(_ colours: [(String, Color)]) -> some View {
        LazyVGrid(columns: Array(repeating: GridItem(.fixed(52), spacing: 6), count: 6), alignment: .leading, spacing: 6) {
            ForEach(colours, id: \.0) { name, c in
                VStack(spacing: 2) {
                    RoundedRectangle(cornerRadius: 8).fill(Ink.line).frame(width: 52, height: 36)
                        .overlay(RoundedRectangle(cornerRadius: 7).fill(c).padding(1))
                    Text(name).type(10, .regular, Ink.detail).lineLimit(1)
                }
            }
        }
    }
}
