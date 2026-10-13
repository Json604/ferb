import AppKit
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// First launch of the packaged FERB.app (L27). It runs the app's own `ferb setup` — the same steps as in a terminal:
/// the background service from inside this app, `ferb` on PATH, permissions guidance and the browser extension — and
/// shows its output in a plain console with one line to answer setup's questions. After the app is replaced by a new
/// build it runs `ferb setup --update` the same way (an ad-hoc build can lose its permissions in an update, and the
/// console says so). Another installation (a checkout, or FERB.app elsewhere) is replaced only after the owner says
/// so. A development build, which has no Python inside, never installs anything.
@MainActor
final class AppSetupController: NSObject, NSWindowDelegate {
    private let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
    private var window: NSWindow?
    private var output: NSTextView?
    private var answer: NSTextField?
    private var process: Process?
    private var answers: FileHandle?
    private var decoder = ConsoleDecoder()
    private var transcript = ""
    private var outputEnded = false                           // finished once the output is read and the process exited
    private var exitStatus: Int32?

    private var python: URL { bundle.appendingPathComponent("Contents/Resources/python/bin/python3.12") }
    private var launcher: URL { bundle.appendingPathComponent("Contents/Resources/bin/ferb") }
    private var agentPlist: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")
    }

    var isPackaged: Bool {
        FileManager.default.isExecutableFile(atPath: launcher.path) && FileManager.default.fileExists(atPath: python.path)
    }

    func startAtLaunch() {
        guard isPackaged else { return }
        let agent = NSDictionary(contentsOf: agentPlist) as? [String: Any]
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        let action = AppSetupPlan.decide(agent: agent, bundlePython: python.path, build: build)
        guard let arguments = AppSetupPlan.arguments(for: action) else { return }
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        run(arguments)
    }

    func runSetupFromMenu() {
        guard isPackaged else { return }
        run(["setup"])
    }

    private func confirmReplacing(_ current: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = "Set up FERB from this app?"
        alert.informativeText = "FERB's background service currently runs \(current). Setting up from this app " +
            "runs it from \(bundle.path) instead. Your data in ~/.ferb stays."
        alert.addButton(withTitle: "Set Up From This App")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertFirstButtonReturn
    }

    private func run(_ arguments: [String]) {
        guard process == nil else { showWindow(); return }
        signal(SIGPIPE, SIG_IGN)                              // an answer typed as setup exits must not end the app
        decoder = ConsoleDecoder()
        transcript = ""
        output?.string = ""
        outputEnded = false
        exitStatus = nil
        let child = Process()
        child.executableURL = launcher
        child.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        environment["PYTHONUNBUFFERED"] = "1"
        child.environment = environment
        let pipe = Pipe()
        child.standardOutput = pipe
        child.standardError = pipe
        let input = Pipe()
        child.standardInput = input
        answers = input.fileHandleForWriting
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil }
            DispatchQueue.main.async {
                guard let self else { return }
                if data.isEmpty { self.outputEnded = true; self.finishIfDone() } else { self.receive(data) }
            }
        }
        child.terminationHandler = { [weak self] finished in
            let status = finished.terminationStatus
            DispatchQueue.main.async { self?.exitStatus = status; self?.finishIfDone() }
        }
        do {
            try child.run()
        } catch {
            show("Couldn't start \(launcher.path): \(error.localizedDescription)\n")
            showWindow()
            return
        }
        process = child
        showWindow()
        answer?.isEnabled = true
    }

    private func receive(_ data: Data) { show(decoder.append(data)) }

    private func show(_ text: String) {
        guard !text.isEmpty else { return }
        transcript += text
        output?.textStorage?.append(NSAttributedString(string: text, attributes: Self.consoleAttributes))
        output?.scrollToEndOfDocument(nil)
    }

    private func finishIfDone() {
        guard outputEnded, let status = exitStatus, process != nil else { return }
        process = nil
        answers = nil
        answer?.isEnabled = false
        let note = status == 0 ? "\nSetup finished.\n" :
            "\nSetup stopped (exit status \(status)). Choose Set Up FERB… in the menu bar to run it again.\n"
        show(note)
    }

    @objc private func send(_ sender: NSTextField) {
        guard let answers else { return }
        let line = sender.stringValue + "\n"
        sender.stringValue = ""
        show(line)                                            // echo it, as a terminal would
        try? answers.write(contentsOf: Data(line.utf8))       // setup may have just exited: no answer is needed
    }

    func windowWillClose(_ notification: Notification) {
        if answers != nil { process?.terminate() }            // the owner closed setup while it was waiting
    }

    private static let consoleAttributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular), .foregroundColor: NSColor.textColor
    ]

    private func showWindow() {
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 720, height: 480),
                                   styleMask: [.titled, .closable, .resizable, .miniaturizable],
                                   backing: .buffered, defer: false)
            created.title = "Set Up \(AppName.current)"
            created.isReleasedWhenClosed = false
            created.delegate = self
            let scroll = NSTextView.scrollableTextView()
            let text = scroll.documentView as? NSTextView
            text?.isEditable = false
            text?.textContainerInset = NSSize(width: 6, height: 6)
            text?.textStorage?.append(NSAttributedString(string: transcript, attributes: Self.consoleAttributes))
            let field = NSTextField()
            field.placeholderString = "Type an answer and press Return (Return alone sends an empty line)"
            field.target = self
            field.action = #selector(send(_:))
            field.isEnabled = answers != nil
            let stack = NSStackView(views: [scroll, field])
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.spacing = 8
            stack.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
            scroll.translatesAutoresizingMaskIntoConstraints = false
            field.translatesAutoresizingMaskIntoConstraints = false
            created.contentView = stack
            NSLayoutConstraint.activate([
                scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -24),
                field.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -24),
                scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 240)
            ])
            window = created
            output = text
            answer = field
            created.center()
        }
        NSApp.activate(ignoringOtherApps: true)               // the owner opened FERB; setup is what they see first
        window?.makeKeyAndOrderFront(nil)
        if answers != nil { window?.makeFirstResponder(answer) }
    }
}
