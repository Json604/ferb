import SwiftUI
import UniformTypeIdentifiers
import NotchCore

struct NotchView: View {
    @ObservedObject var model: NotchModel
    @ObservedObject var presentation: NotchPresentation
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack(alignment: .top) {
            NotchSurfaceShape(expansion: presentation.expansion, docked: model.isNotchDocked)
                .fill(Palette.green.opacity(model.isDropTarget ? 0.1 : 0))
                .frame(width: presentation.size.width, height: presentation.size.height)
                .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
            VStack(spacing: 0) {
                if case .intro(let line)? = model.compactContent {
                    IntroNarrationView(line: line, cameraSpace: model.isNotchDocked ? model.cutoutHeight : 0,
                                       act: model.introActions, typed: $model.introTyped,
                                       canSkip: model.introSkippable)
                        .frame(height: presentation.size.height, alignment: .top)
                } else if model.compactContent == .pulse {
                    Color.clear.frame(height: presentation.size.height)
                } else if case .reminder(let card, let text)? = model.compactContent {
                    // Its buttons (Done; a timer's Stop and Repeat, APP-762) take their own clicks; anywhere else opens
                    // today's chat as before (APP-622, APP-624)
                    ReminderArrivalSurface(card: card, text: text, model: model)
                        .padding(.top, model.isNotchDocked ? model.cutoutHeight : 0)
                        .frame(height: presentation.size.height, alignment: .top)
                } else if case .plan(let text)? = model.compactContent {
                    ScrollView {
                        Text(text)
                            .font(.system(size: 13))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 25)
                            .padding(.vertical, 17)
                    }
                    .padding(.top, model.isNotchDocked ? model.cutoutHeight : 0)
                    .frame(height: presentation.size.height)
                    .contentShape(Rectangle())
                    .pointingHand("plan")
                    .onTapGesture { model.expand() }
                    .accessibilityLabel("FERB is planning. Open chat.")
                } else if model.indicatorHeaderAtBottom {
                    bodyContent
                    header
                } else {
                    header
                    bodyContent
                }
            }
            // Keep the full body mounted without letting its minimum height
            // recenter the header when AppKit trims the canvas after a fold.
            .fixedSize(horizontal: false, vertical: true)
            .frame(width: model.bodyWidth, height: presentation.size.height, alignment: .top)
            .opacity(presentation.expansion)
            .mask(alignment: .top) {
                NotchSurfaceShape(expansion: presentation.expansion, docked: model.isNotchDocked)
                    .frame(width: presentation.size.width, height: presentation.size.height)
            }
            .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
            .allowsHitTesting(presentation.expansion > 0.001 && model.surfaceVisible)
            .accessibilityHidden(!model.surfaceVisible)
            if presentation.expansion < 0.001 {
                Button { model.toggle() } label: {
                    Color.clear.frame(width: model.handleWidth, height: model.cutoutHeight)
                        .contentShape(Rectangle())
                }
                .offset(x: presentation.surfaceOffset.x, y: presentation.surfaceOffset.y)
                .buttonStyle(.plain)
                .accessibilityLabel(model.session.phase == .working && !model.isStanding(model.session)
                                    ? "FERB is working. Open activity." : "Open FERB")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .environment(\.selectableTextGeneration, model.textRenderGeneration)
        .foregroundStyle(Palette.ink)
        .preferredColorScheme(.dark)
        .ignoresSafeArea()
        .onDrop(of: [UTType.fileURL], delegate: ChatFileDrop(session: model.drops))
        .onExitCommand { model.collapse(keyboard: true) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("FERB notch")
    }

    /// The chat body while the notch holds the chat; while the sticky window holds it (APP-745) the notch mounts none.
    @ViewBuilder private var bodyContent: some View {
        if model.chatInWindow {
            Color.clear.frame(height: 0)
        } else {
            notchBody
        }
    }

    private var notchBody: some View {
        ChatBodyView(model: model, animates: presentation.animates)
            .padding(.horizontal, 25)
            .padding(.top, 9)
            .padding(.bottom, 17)
            .frame(height: model.bodyHeight)
            .scaleEffect(reduceMotion ? 1 : 0.975 + 0.025 * presentation.expansion, anchor: .top)
            .blur(radius: reduceMotion ? 0 : 2 * (1 - presentation.expansion))
            .frame(height: model.bodyHeight, alignment: .top)
            .clipped()
            .allowsHitTesting(model.bodyVisible)
            .accessibilityHidden(!model.bodyVisible)
    }

    private var header: some View {
        HStack(spacing: 0) {
            ChatTitle(model: model)
                .frame(width: model.isNotchDocked ? max(0, (model.bodyWidth - model.handleWidth) / 2 - 18) : nil,
                       alignment: .leading)
            Spacer(minLength: 0)
            if model.isNotchDocked {
                // Keep the original camera safe area clear; title and account live in its wings.
                Color.clear.frame(width: model.handleWidth).accessibilityHidden(true)
                Spacer(minLength: 0)
            }
            ProfileMenuButton(model: model).frame(width: 26, height: 26)
                .frame(width: model.isNotchDocked ? max(0, (model.bodyWidth - model.handleWidth) / 2 - 18) : nil,
                       alignment: .trailing)
        }
        .padding(.horizontal, 18)
        .frame(height: max(38, model.cutoutHeight))
    }
}
