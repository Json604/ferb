import AppKit
import Sparkle
import NotchCore

/// Sparkle 2 for FERB.app (L34 Phase G): its standard updater over FERB's public release feed, about once a day
/// (`SUScheduledCheckInterval`). With automatic updates on, Sparkle downloads and verifies an update silently and hands
/// its install to UpdateCoordinator, which waits until FERB is idle. With them off, FERB shows "Update available —
/// Install" instead of Sparkle's window (gentle reminders: a background app never takes focus by itself, rule 17).
@MainActor
final class SparkleUpdater: NSObject, UpdaterDriving, SPUUpdaterDelegate, SPUStandardUserDriverDelegate {
    private let feed: String
    private weak var coordinator: UpdateCoordinator?
    private var controller: SPUStandardUpdaterController?

    /// nil for a build without FERB's release feed and key (a development build, or FERB_UPDATES=off).
    init?(coordinator: UpdateCoordinator, info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        guard let feed = UpdatePlan.feedURL(info) else { return nil }
        self.feed = feed
        self.coordinator = coordinator
        super.init()
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self,
                                                  userDriverDelegate: self)
    }

    func start() {
        guard let controller else { return }
        controller.updater.automaticallyChecksForUpdates = true    // checks stay on; the switch decides installing
        controller.startUpdater()
    }

    var automaticallyDownloadsUpdates: Bool {
        get { controller?.updater.automaticallyDownloadsUpdates ?? true }
        set { controller?.updater.automaticallyDownloadsUpdates = newValue }
    }

    func checkForUpdates() { controller?.checkForUpdates(nil) }

    // MARK: SPUUpdaterDelegate (Sparkle calls these on the main thread)

    /// FERB's own feed only: Sparkle would read a `SUFeedURL` preference before Info.plist; this answer comes first.
    nonisolated func feedURLString(for updater: SPUUpdater) -> String? { feed }

    nonisolated func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.found(version: version) }
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
        let message = error.localizedDescription
        MainActor.assumeIsolated { coordinator?.aborted(message) }
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
        MainActor.assumeIsolated { coordinator?.found(version: version) }
    }
}
