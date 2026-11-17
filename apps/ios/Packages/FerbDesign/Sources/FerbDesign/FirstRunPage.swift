import SwiftUI

// Not from the lab: the first run's asks (kb PHN-636; Android's PermissionsPage, one look), built from the pair page's
// pieces: the white card at the bottom over the sky, PairStep's grey circles, the black pill.

/// One thing the first run asks the phone for: its SF Symbol, its name and why FERB wants it, in a line.
public struct AskRow: Hashable, Sendable {
    public let symbol: String
    public let title: String
    public let why: String
    public init(symbol: String, title: String, why: String) {
        self.symbol = symbol
        self.title = title
        self.why = why
    }
}

/// What FERB will ask the phone for and why, a line each, one Continue that brings the system's questions one after
/// another, and Not now. Stateless: the app passes its words.
public struct PermissionsPage: View {
    let title: String
    let intro: String
    let rows: [AskRow]
    let continueTitle: String
    let notNowTitle: String
    var insets: PhoneInsets
    var onContinue: (() -> Void)?
    var onNotNow: (() -> Void)?

    public init(title: String, intro: String, rows: [AskRow], continueTitle: String, notNowTitle: String,
                insets: PhoneInsets = .lab, onContinue: (() -> Void)? = nil, onNotNow: (() -> Void)? = nil) {
        self.title = title
        self.intro = intro
        self.rows = rows
        self.continueTitle = continueTitle
        self.notNowTitle = notNowTitle
        self.insets = insets
        self.onContinue = onContinue
        self.onNotNow = onNotNow
    }

    public var body: some View {
        VStack(spacing: 0) {
            Spacer()
            Card(padding: 20) {
                VStack(alignment: .leading, spacing: 14) {
                    Text(title).type(24, .semibold).accessibilityAddTraits(.isHeader)
                    Text(intro).type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    ForEach(rows, id: \.self) { row in
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: row.symbol).font(.system(size: 15, weight: .medium)).foregroundStyle(Ink.black)
                                .frame(width: 34, height: 34).background(Ink.fill, in: Circle())
                            VStack(alignment: .leading, spacing: 2) {
                                Text(row.title).type(15.5, .semibold)
                                Text(row.why).type(14, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                    Pill(title: continueTitle, kind: .black, onClick: onContinue).padding(.top, 4)
                    Text(notNowTitle).type(15, .medium, Ink.detail)
                        .padding(.horizontal, 16).padding(.vertical, 6)
                        .pressable(onNotNow, label: notNowTitle)
                        .frame(maxWidth: .infinity)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, insets.bottom + 8)
        }
    }
}
