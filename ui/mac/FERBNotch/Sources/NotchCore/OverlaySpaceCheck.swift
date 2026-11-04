/// Whether FERB's notch windows really are in its fixed overlay space (APP-563, APP-704), read from what the window
/// server lists, never from the SkyLight calls' return codes: on macOS 27.2 SLSSpaceAddWindowsAndRemoveFromSpaces and
/// SLSShowSpaces return 3 even when every window joined the space and it shows. Reading those codes made FERB give the
/// space up on every launch, so its handle became an ordinary window on the menu bar's own layer, which macOS 27's
/// menu-bar window covers at the camera: the docked handle took no click or drag (owner, v0.27.0).
public enum OverlaySpaceCheck {
    /// - Parameters:
    ///   - windows: the window numbers moved into the space.
    ///   - levelStatus: SLSSpaceSetAbsoluteLevel's result. Nothing moves into a space whose level didn't take, so this
    ///     one result still counts.
    ///   - members: the window numbers the space lists after the move.
    ///   - shownBefore: those of `windows` that were on screen before the move.
    ///   - onScreen: the window numbers listed on screen after the space was shown. A space that doesn't show lists
    ///     none of its windows there, so a window that was on screen and no longer is means the space isn't showing.
    public static func attached(windows: [Int], levelStatus: Int32, members: Set<Int>, shownBefore: [Int],
                                onScreen: Set<Int>) -> Bool {
        levelStatus == 0 && !windows.isEmpty && windows.allSatisfy(members.contains)
            && shownBefore.allSatisfy(onScreen.contains)
    }
}
