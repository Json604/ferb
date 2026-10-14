// FERB's home for FERB Voice (macapp/FERB): the account's own ~/.ferb, found in the account database (never $HOME).
// Whoever starts FERB Voice can set its environment (`open -n --env FERB_HOME=…`), and FERB Voice holds Microphone and
// Speech Recognition, so FERB_HOME is taken only when it is unset (or empty) or names that same folder by device and
// inode, as FERB.app's agent does (ui/mac/FERBAgent/main.c). Anything else is refused before FERB Voice does anything:
// no other process can point it at its own socket or audio folder.
import Darwin

enum FerbHome {
    case home(String)
    case refused(String)
}

/// The account's ~/.ferb from the account database; nil when the account has no record.
func accountFerbHome() -> String? {
    guard let pw = getpwuid(getuid()), let dir = pw.pointee.pw_dir else { return nil }
    return String(cString: dir) + "/.ferb"
}

/// `given`: FERB_HOME as FERB Voice was started with it (nil: unset). `account`: the account's own ~/.ferb.
func resolveFerbHome(_ given: String?, account: String?) -> FerbHome {
    guard let account = account else { return .refused("FERB Voice can't find your account's home folder") }
    guard let given = given, !given.isEmpty else { return .home(account) }
    var a = stat(), b = stat()
    guard stat(given, &a) == 0, stat(account, &b) == 0, a.st_dev == b.st_dev, a.st_ino == b.st_ino else {
        return .refused("FERB_HOME isn't your ~/.ferb: FERB Voice runs only with your own FERB home")
    }
    return .home(account)                         // the account's own path, never the spelling it was given as
}

/// Checked once, on first use; main.swift uses it before anything else. A refusal exits (64, as the agent does).
let ferbHome: String = {
    let given = getenv("FERB_HOME").map { String(cString: $0) }
    switch resolveFerbHome(given, account: accountFerbHome()) {
    case .home(let home):
        return home
    case .refused(let why):
        fputs(why + "\n", stderr)
        exit(64)
    }
}()
