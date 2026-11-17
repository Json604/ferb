import AppKit
import Sparkle
import NotchCore

/// Sparkle 2 for FERB.app (L34 Phase G): its updater over FERB's public release feed, about once a day
/// (`SUScheduledCheckInterval`). With automatic updates on, Sparkle downloads and verifies an update silently and hands
/// its install to UpdateCoordinator, which waits until FERB is idle. With them off, FERB shows "Update available —
/// Install" instead of Sparkle's window (gentle reminders: a background app never takes focus by itself, rule 17).
///
/// APP-945..947: Check for Updates… is Sparkle's probing check (`checkForUpdateInformation`, no window and no alert):
/// its answer comes back through the delegate calls below and shows as Settings' line. Sparkle's own windows (the
/// update window with its release notes, the download and ready-to-install window) open only for the owner's Install,
/// and FERB's user driver (`FERBUserDriver`) answers the three places Sparkle's standard driver would run a modal alert
/// (an error, no update found, installed without a relaunch) in FERB's own UI instead. Built without
/// SPUStandardUpdaterController, whose start-up failure is a modal alert too.
@MainActor
final class SparkleUpdater: NSObject, UpdaterDriving, SPUUpdaterDelegate, SPUStandardUserDriverDelegate {
    /// REL-431, REL-432: the addresses this build may ask, in order (the owner's domain, then the releases' own), and
    /// the one being asked. Sparkle reads `feedURLString` on the main thread, as it calls every delegate method.
    private let feeds: [String]
    nonisolated(unsafe) private var asking = 0
    /// The address just asked couldn't be had and another is left: the next one is asked when this cycle ends.
    nonisolated(unsafe) private var askNext = false
    private weak var coordinator: UpdateCoordinator?
    private var updater: SPUUpdater?
    private var userDriver: FERBUserDriver?
    private var started = false

    /// nil for a build without FERB's release feed and key (a development build, or FERB_UPDATES=off).
    init?(coordinator: UpdateCoordinator, info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        let feeds = UpdatePlan.feeds(info)
        guard !feeds.isEmpty else { return nil }
        self.feeds = feeds
        self.coordinator = coordinator
        super.init()
        let standard = SPUStandardUserDriver(hostBundle: .main, delegate: self)
        let driver = FERBUserDriver(standard: standard)
        userDriver = driver
        updater = SPUUpdater(hostBundle: .main, applicationBundle: .main, userDriver: driver, delegate: self)
    }

    func start() {
        guard let updater else { return }
        updater.automaticallyChecksForUpdates = true    // checks stay on; the switch decides installing
        do {
            try updater.start()
            started = true
        } catch {
            // Sparkle's controller would put up a modal alert here; FERB keeps the reason in Settings' last result
            coordinator?.aborted(UpdatePlan.reason(error as NSError))
        }
    }

    var automaticallyDownloadsUpdates: Bool {
        get { updater?.automaticallyDownloadsUpdates ?? true }
        set { updater?.automaticallyDownloadsUpdates = newValue }
    }

    /// Check for Updates…: no window, no alert (APP-946). An update session already running (a scheduled check) gives
    /// its own answer through the same delegate calls; an updater that never started gives none.
    func checkQuietly() -> QuietCheck {
        guard let updater, started else { return .unavailable }
        guard !updater.sessionInProgress else { return .alreadyRunning }
        updater.checkForUpdateInformation()
        return .started
    }

    /// The owner's Install for an update FERB offered: Sparkle's update window, on the Space showing (APP-796).
    func installOffered() {
        guard let updater else { return }
        AppWindowPresentation.opening("the update window", for: 10) { updater.checkForUpdates() }
    }

    // MARK: SPUUpdaterDelegate (Sparkle calls these on the main thread)

    /// FERB's own feed only: Sparkle would read a `SUFeedURL` preference before Info.plist; this answer comes first.
    nonisolated func feedURLString(for updater: SPUUpdater) -> String? { feeds[min(asking, feeds.count - 1)] }

    /// A major upgrade (the appcast's minimumAutoupdateVersion is newer than this build: the switch to the Developer
    /// ID, REL-423) is one Sparkle never installs in the background; FERB asks the owner first (APP-903).
    nonisolated func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        let version = item.displayVersionString
        let major = item.isMajorUpgrade
        MainActor.assumeIsolated { coordinator?.found(version: version, asksFirst: major) }
    }

    nonisolated func updaterDidNotFindUpdate(_ updater: SPUUpdater) {
        MainActor.assumeIsolated { coordinator?.notFound() }
    }

    /// An automatic update is downloaded and verified: FERB installs it when idle (UpdateCoordinator).
    nonisolated func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem,
                             immediateInstallationBlock immediateInstallHandler: @escaping () -> Void) -> Bool {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.readyToInstall(version: version, install: immediateInstallHandler) }
        return true
    }

    /// The owner chose Install in Sparkle's window: the same wait for an idle FERB before the relaunch.
    nonisolated func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                             untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.readyToInstall(version: version, install: installHandler) }
        return true
    }

    nonisolated func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        let failure = error as NSError
        // "No update found" arrives here too (updaterDidNotFindUpdate records it), as does the owner's own cancel.
        guard UpdatePlan.abortIsFailure(domain: failure.domain, code: failure.code) else { return }
        // REL-432: the feed couldn't be had at this address and the list has another: ask it before saying anything
        if UpdatePlan.feedFailed(domain: failure.domain, code: failure.code),
           let next = UpdatePlan.nextFeed(after: asking, of: feeds) {
            asking = next
            askNext = true
            return
        }
        asking = 0                                   // the next check starts from the owner's domain again
        let message = UpdatePlan.reason(failure)
        MainActor.assumeIsolated { coordinator?.aborted(message) }
    }

    /// Every update session ends here, a quiet check's included: whatever it answered, FERB stops showing "Checking".
    nonisolated func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck,
                             error: Error?) {
        if askNext {
            askNext = false
            MainActor.assumeIsolated { self.updater?.checkForUpdateInformation() }   // quiet, as every check is
            return
        }
        MainActor.assumeIsolated { coordinator?.checkFinished() }
    }

    // MARK: SPUStandardUserDriverDelegate

    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    nonisolated func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem,
                                                                           andInImmediateFocus immediateFocus: Bool) -> Bool {
        false                                            // FERB shows "Update available — Install" itself
    }

    nonisolated func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool,
                                                               forUpdate update: SUAppcastItem,
                                                               state: SPUUserUpdateState) {
        guard !handleShowingUpdate else { return }
        let version = update.displayVersionString
        let major = update.isMajorUpgrade
        MainActor.assumeIsolated { coordinator?.found(version: version, asksFirst: major) }
    }
}

/// Sparkle's standard user driver with its three modal alerts taken out (APP-947): every window it shows (the update
/// window and its release notes, the checking, download and ready-to-install window) is the standard one, while an
/// error, "no update found" and "installed" without a relaunch close Sparkle's windows and go on at once. Their facts
/// already reach FERB through the updater's delegate (didAbortWithError, updaterDidNotFindUpdate), which shows them in
/// Settings and the chat. Sparkle's private calls to its standard driver (gentle-reminder timing) are forwarded.
@MainActor
final class FERBUserDriver: NSObject, SPUUserDriver {
    nonisolated(unsafe) private let standard: SPUStandardUserDriver

    init(standard: SPUStandardUserDriver) {
        self.standard = standard
        super.init()
    }

    nonisolated override func responds(to selector: Selector!) -> Bool {
        super.responds(to: selector) || standard.responds(to: selector)
    }

    nonisolated override func forwardingTarget(for selector: Selector!) -> Any? {
        standard.responds(to: selector) ? standard : nil
    }

    // The three alerts (SPUStandardUserDriver's -showAlert:secondaryAction:, a runModal): never shown

    func showUpdateNotFoundWithError(_ error: Error, acknowledgement: @escaping () -> Void) {
        standard.dismissUpdateInstallation()
        acknowledgement()
    }

    func showUpdaterError(_ error: Error, acknowledgement: @escaping () -> Void) {
        standard.dismissUpdateInstallation()
        acknowledgement()
    }

    func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        if relaunched {
            standard.showUpdateInstalledAndRelaunched(true, acknowledgement: acknowledgement)   // no alert then
        } else {
            standard.dismissUpdateInstallation()
            acknowledgement()
        }
    }

    // Everything else is the standard driver's

    func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        standard.show(request, reply: reply)
    }

    func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {
        standard.showUserInitiatedUpdateCheck(cancellation: cancellation)
    }

    func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState,
                         reply: @escaping (SPUUserUpdateChoice) -> Void) {
        standard.showUpdateFound(with: appcastItem, state: state, reply: reply)
    }

    func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {
        standard.showUpdateReleaseNotes(with: downloadData)
    }

    func showUpdateReleaseNotesFailedToDownloadWithError(_ error: Error) {
        standard.showUpdateReleaseNotesFailedToDownloadWithError(error)
    }

    func showDownloadInitiated(cancellation: @escaping () -> Void) {
        standard.showDownloadInitiated(cancellation: cancellation)
    }

    func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        standard.showDownloadDidReceiveExpectedContentLength(expectedContentLength)
    }

    func showDownloadDidReceiveData(ofLength length: UInt64) {
        standard.showDownloadDidReceiveData(ofLength: length)
    }

    func showDownloadDidStartExtractingUpdate() { standard.showDownloadDidStartExtractingUpdate() }

    func showExtractionReceivedProgress(_ progress: Double) { standard.showExtractionReceivedProgress(progress) }

    func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        standard.showReady(toInstallAndRelaunch: reply)
    }

    func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool,
                              retryTerminatingApplication: @escaping () -> Void) {
        standard.showInstallingUpdate(withApplicationTerminated: applicationTerminated,
                                      retryTerminatingApplication: retryTerminatingApplication)
    }

    func dismissUpdateInstallation() { standard.dismissUpdateInstallation() }

    func showUpdateInFocus() { standard.showUpdateInFocus() }
}
