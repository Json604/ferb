import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI
import UIKit

/// The ways the Mac takes (the contract's `variant`).
let shieldWay = "shield"
let lockedWay = "locked"

/// The Mac tab's "Keep my Mac working" (APP-980; Android's InsomniaCard): the switch (on opens the warning, off ends
/// it), the Mac's own line and End while it is on, someone at the Mac with Show me, Lock my Mac and It's fine (APP-983),
/// and the warnings (APP-981). Shown only when the Mac says it can (an older Mac has no `insomnia`).
struct InsomniaCard: View {
    @Bindable var model: AppModel
    let i: Insomnia

    var body: some View {
        let busy = model.ui.busy
        let ending = busy.contains("insomnia-end")
        Card(padding: 16) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .center, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(L.s("insomnia_row")).type(16.5, .semibold, Ink.black)
                        if i.on && !i.line.isEmpty {
                            Text(i.line).type(14, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                                .accessibilityAddTraits(.updatesFrequently)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    PhoneSwitch(on: i.on, onChange: ending || busy.contains("insomnia") ? nil : { on in
                        if on { model.askInsomnia() } else { model.endInsomnia() }
                    }, label: L.s("insomnia_row"))
                }
                if i.on, let n = i.notice {
                    Text(n.text).type(15.5, .medium, Ink.black).fixedSize(horizontal: false, vertical: true).padding(.top, 12)
                    HStack(spacing: 8) {
                        Pill(title: L.s("insomnia_show_me"), kind: .black, symbol: "camera", height: 44) { model.showCamera() }
                        Pill(title: L.s("insomnia_lock_mac"), kind: .grey, symbol: "lock", height: 44,
                             onClick: busy.contains("lock-mac") ? nil : { model.lockMac() })
                    }
                    .padding(.top, 10)
                    Pill(title: L.s("insomnia_fine"), kind: .plain, height: 40,
                         onClick: busy.contains("insomnia-fine") ? nil : { model.insomniaFine(n.id) })
                        .padding(.top, 4)
                }
                if i.on {
                    if let problem = MacAsleep.helperProblem(i) {
                        Text(problem).type(14.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true).padding(.top, 10)
                    }
                    ForEach(Array(i.warnings.enumerated()), id: \.offset) { _, w in
                        Text(w.text).type(14.5, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true).padding(.top, 10)
                    }
                    Pill(title: L.s("insomnia_end"), kind: .grey, height: 44, onClick: ending ? nil : { model.endInsomnia() })
                        .padding(.top, 12)
                }
            }
        }
        .padding(.horizontal, 16).padding(.bottom, 12)
    }
}

/// The warning before the phone turns it on (APP-980; Android's InsomniaSheet): the same words as the Mac's dialog,
/// the two ways (Cover the screen by default), the one Yes under the first (on by default, APP-984), when it ends, Keep
/// it working (Face ID / Touch ID signs it) and Not now.
struct InsomniaSheet: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    @State private var way = shieldWay
    @State private var screenYes = true

    var body: some View {
        let close = { model.show(nil) }
        let minutes = Int(model.ui.status?.insomnia?.graceMinutes ?? 10)
        Sheet(bottomInset: insets.bottom, onDismiss: close) {
            SheetTitle(L.s("insomnia_title"), onClose: close)
            // the closed-lid sentence is the Mac's own, true to its helper's state (an older Mac: the app's words)
            Text(MacAsleep.body(open: L.s("insomnia_body_open"), fallback: L.s("insomnia_body_closed"), model.ui.status?.insomnia))
                .type(15.5, .regular, Ink.detail).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            // why the helper that keeps a closed Mac running isn't on (the Mac's words), under the lid line
            if let problem = MacAsleep.helperProblem(model.ui.status?.insomnia) {
                Text(problem).type(14.5, .medium, Ink.black).fixedSize(horizontal: false, vertical: true).padding(.top, 8)
            }
            VStack(spacing: 0) {
                choice(L.s("insomnia_shield"), L.s("insomnia_shield_text"), shieldWay)
                if way == shieldWay {
                    Button { screenYes.toggle() } label: {
                        HStack(spacing: 10) {
                            Image(systemName: screenYes ? "checkmark.square.fill" : "square").font(.system(size: 20)).foregroundStyle(Ink.black)
                            Text(L.s("insomnia_screen_yes")).type(14.5, .medium, Ink.black).fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 14).padding(.bottom, 12)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityValue(Text(screenYes ? "On" : "Off"))
                }
                Divide()
                choice(L.s("insomnia_locked"), L.s("insomnia_locked_text"), lockedWay)
            }
            .background(Ink.fill, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .padding(.top, 14)
            Text(L.s("insomnia_ends", minutes)).type(14, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true).padding(.top, 12)
            Pill(title: L.s("insomnia_go"), kind: .black, symbol: "faceid",
                 onClick: model.ui.busy.contains("insomnia") ? nil : { model.startInsomnia(variant: way, screenYes: way == shieldWay && screenYes) })
                .padding(.top, 16)
            Pill(title: L.s("insomnia_not_now"), kind: .plain, height: 44, onClick: close).padding(.top, 4)
        }
    }

    /// One of the two ways: its name, what it means, a check when chosen.
    private func choice(_ title: String, _ text: String, _ value: String) -> some View {
        Button { way = value } label: {
            HStack(alignment: .center, spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).type(16.5, .semibold, Ink.black)
                    Text(text).type(14, .regular, Ink.detail).fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "checkmark").font(.system(size: 17, weight: .semibold)).foregroundStyle(Ink.black)
                    .opacity(way == value ? 1 : 0)
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(way == value ? .isSelected : [])
    }
}

/// Show me (APP-983): the Mac's camera while this shows. With the owner's screenshot switch on, the picture is kept out
/// of screenshots where this iOS allows it (the secure canvas) and the app is hidden while the screen is captured (PHN-811).
struct CameraView: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets

    var body: some View {
        if let v = model.camera.view {
            let image = v.frame.flatMap { UIImage(data: $0) }
            let notice = model.ui.status?.insomnia?.notice
            VStack(spacing: 0) {
                HStack {
                    Text(L.s("camera_title")).type(22, .semibold, Ink.white).accessibilityAddTraits(.isHeader)
                    Spacer()
                    CloseButton { model.closeCamera() }
                }
                .padding(.horizontal, 16)
                ZStack {
                    if let image {
                        Image(uiImage: image).resizable().scaledToFit().screenshotShielded()
                            .accessibilityLabel(Text(L.s("camera_title")))
                    } else {
                        Text(v.ended?.text ?? v.text ?? L.s("camera_starting")).type(16, .regular, Ink.onDark)
                            .multilineTextAlignment(.center).padding(.horizontal, 16)
                            .accessibilityAddTraits(.updatesFrequently)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(16)
                VStack(spacing: 8) {
                    HStack(spacing: 8) {
                        Pill(title: L.s("insomnia_lock_mac"), kind: .grey, symbol: "lock", height: 46,
                             onClick: model.ui.busy.contains("lock-mac") ? nil : { model.lockMac() })
                        if let n = notice {
                            Pill(title: L.s("insomnia_fine"), kind: .grey, height: 46) {
                                model.insomniaFine(n.id)
                                model.closeCamera()
                            }
                        }
                    }
                    if v.ended != nil {
                        Pill(title: L.s("live_again"), kind: .grey, symbol: "eye", height: 46) { model.cameraAgain() }
                    } else {
                        Pill(title: L.s("live_close"), kind: .grey, height: 46) { model.closeCamera() }
                    }
                }
                .padding(.horizontal, 16)
            }
            .padding(.top, insets.top + 8).padding(.bottom, insets.bottom + 8)
            .background(Color.black.ignoresSafeArea())
            .environment(\.onFerb, true)
            // opened over the page, it rises and fades in, and fades out (kb PHN-623)
            .presenceFade()
        }
    }
}
