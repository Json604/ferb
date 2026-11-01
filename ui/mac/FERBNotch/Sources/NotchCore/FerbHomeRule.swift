// FERB's home for the hearing modes (Sources/FERBNotch/Hearing.swift): the account's own ~/.ferb, found in the account
// database (never $HOME). Whoever starts FERB.app's hearing can set its environment (`open -n --env FERB_HOME=…`), and
// FERB.app holds Microphone and Speech Recognition, so FERB_HOME is taken only when it is unset (or empty) or names that
// same folder by device and inode, as FERB.app's agent does (ui/mac/FERBAgent/main.c). Anything else is refused before
// hearing does anything: no other process can point it at its own socket or audio folder.
import Darwin

public enum FerbHome: Equatable {
    case home(String)
    case refused(String)
}

/// The account's ~/.ferb from the account database; nil when the account has no record.
public func accountFerbHome() -> String? {
    guard let pw = getpwuid(getuid()), let dir = pw.pointee.pw_dir else { return nil }
    return String(cString: dir) + "/.ferb"
}

/// `given`: FERB_HOME as hearing was started with it (nil: unset). `account`: the account's own ~/.ferb.
public func resolveFerbHome(_ given: String?, account: String?) -> FerbHome {
    guard let account = account else { return .refused("FERB can't find your account's home folder") }
    guard let given = given, !given.isEmpty else { return .home(account) }
    var a = stat(), b = stat()
    guard stat(given, &a) == 0, stat(account, &b) == 0, a.st_dev == b.st_dev, a.st_ino == b.st_ino else {
        return .refused("FERB_HOME isn't your ~/.ferb: FERB's hearing runs only with your own FERB home")
    }
    return .home(account)                         // the account's own path, never the spelling it was given as
}
