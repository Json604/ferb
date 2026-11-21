import AppKit
import NotchCore

/// Insomnia mode's warning on the Mac (APP-980): only the owner turns the mode on, after it. The same words as the
/// phone's sheet (InsomniaWords). Cover the screen is the default choice, with the one Yes (APP-984) checked under it;
/// Keep it working starts the mode with the owner's choice, Not now does nothing.
@MainActor
enum InsomniaDialog {
    /// Shows the warning and, on Keep it working, starts the mode through `model`. `helper`: the packaged app's root
    /// helper (APP-986), registered first when macOS doesn't list it; nil (a development build): the Mac sleeps with a
    /// closed lid on battery, as the line says.
    static func run(_ model: NotchModel, helper: PowerHelper?) {
        Task { @MainActor in
            let closedLid = await helper?.prepare() ?? .sleeps
            await model.refreshInsomnia()        // the daemon's lid line and the helper's problem, after that attempt
            show(model, helper: helper, closedLid: closedLid)
        }
    }

    private static func show(_ model: NotchModel, helper: PowerHelper?, closedLid: PowerHelperPlan.ClosedLid) {
        let minutes = model.insomniaStatus?.graceMinutes ?? InsomniaStatus.defaultGraceMinutes
        let form = Form()
        let alert = NSAlert()
        alert.messageText = InsomniaWords.title
        alert.informativeText = informative(minutes: minutes, lidLine: InsomniaWords.lidLine(model.insomniaStatus,
                                                                                               local: closedLid),
                                            problem: model.insomniaStatus?.helper?.problem)
        alert.accessoryView = form.view
        for title in buttons(closedLid) { alert.addButton(withTitle: title) }
        switch AppWindowPresentation.runModal(alert) {                                   // on this Space (APP-796)
        case .alertFirstButtonReturn: model.startInsomnia(form.choice)
        case .alertThirdButtonReturn: helper?.openLoginItems()       // he turns it on, then turns the mode on again
        default: break
        }
    }

    /// The contract's body, the closed lid's line under it (the daemon's `lid_line`) and the helper's problem when there
    /// is one, then when it ends.
    static func informative(minutes: Int, lidLine: String, problem: String? = nil) -> String {
        InsomniaWords.body + "\n" + lidLine + (problem.map { "\n" + $0 } ?? "") + "\n\n" + InsomniaWords.ends(minutes: minutes)
    }

    /// Keep it working (the default), Not now, and Open Login Items… while the helper waits for his switch.
    static func buttons(_ closedLid: PowerHelperPlan.ClosedLid) -> [String] {
        [InsomniaWords.keepWorking, InsomniaWords.notNow] + (closedLid == .needsApproval ? [InsomniaWords.openLoginItems] : [])
    }

    /// The two choices and the checkbox.
    @MainActor
    final class Form: NSObject {
        let cover = NSButton(radioButtonWithTitle: InsomniaWords.coverChoice, target: nil, action: nil)
        let lock = NSButton(radioButtonWithTitle: InsomniaWords.lockChoice, target: nil, action: nil)
        let screenYes = NSButton(checkboxWithTitle: InsomniaWords.screenYes, target: nil, action: nil)
        let view: NSView

        override init() {
            func detail(_ text: String) -> NSTextField {
                let label = NSTextField(wrappingLabelWithString: text)
                label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
                label.textColor = .secondaryLabelColor
                label.preferredMaxLayoutWidth = 320
                return label
            }
            let stack = NSStackView()
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.spacing = 6
            let coverDetail = detail(InsomniaWords.coverDetail), lockDetail = detail(InsomniaWords.lockDetail)
            for item in [cover, coverDetail, screenYes, lock, lockDetail] as [NSView] { stack.addArrangedSubview(item) }
            stack.setCustomSpacing(14, after: screenYes)
            stack.translatesAutoresizingMaskIntoConstraints = false
            let container = NSView(frame: NSRect(x: 0, y: 0, width: 340, height: 10))
            container.addSubview(stack)
            // each set active on its own (the Space rule's scan reads every `activate(` as FERB activating)
            let constraints: [NSLayoutConstraint] = [
                stack.leadingAnchor.constraint(equalTo: container.leadingAnchor),
                stack.trailingAnchor.constraint(equalTo: container.trailingAnchor),
                stack.topAnchor.constraint(equalTo: container.topAnchor),
                stack.bottomAnchor.constraint(equalTo: container.bottomAnchor),
                container.widthAnchor.constraint(equalToConstant: 340),
            ]
            constraints.forEach { $0.isActive = true }
            container.layoutSubtreeIfNeeded()
            container.setFrameSize(stack.fittingSize)
            view = container
            super.init()
            for button in [cover, lock] {
                button.target = self
                button.action = #selector(chose(_:))
            }
            cover.state = .on
            screenYes.state = .on
        }

        @objc private func chose(_ sender: NSButton) {
            cover.state = sender === cover ? .on : .off
            lock.state = sender === lock ? .on : .off
            screenYes.isEnabled = cover.state == .on       // the Yes is about apps under the cover
        }

        /// What the owner chose (and what `insomnia.start` is sent).
        var choice: InsomniaChoice {
            InsomniaChoice(variant: lock.state == .on ? .locked : .shield, screenYes: screenYes.state == .on)
        }

        /// Picks a choice as a click does (tests).
        func pick(_ variant: InsomniaChoice.Variant) { chose(variant == .shield ? cover : lock) }
    }
}
