import Darwin
import FerbAppModel
import Foundation

/// The start-up's milestones under `ferb-app` (kb PHN-633; Android's StartLog), each in ms since the process began,
/// so a start on a real iPhone can be read in Console: the app, the first frame, the first page the gate allows, the
/// phone opened (library, keys, link and store, off the main thread) and the launch settled. Each once per process.
@MainActor
enum StartLog {
    private static var seen = Set<String>()

    /// When this process began (the kernel's record), or now when it can't be read.
    static let processStart: Date = {
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&mib, 4, &info, &size, nil, 0) == 0 else { return Date() }
        let t = info.kp_proc.p_un.__p_starttime
        return Date(timeIntervalSince1970: Double(t.tv_sec) + Double(t.tv_usec) / 1_000_000)
    }()

    static func mark(_ what: String, _ detail: String = "") {
        guard seen.insert(what).inserted else { return }
        let ms = Int(Date().timeIntervalSince(processStart) * 1000)
        AppLog.i("start: \(what) at \(ms) ms\(detail.isEmpty ? "" : " (\(detail))")")
    }
}
