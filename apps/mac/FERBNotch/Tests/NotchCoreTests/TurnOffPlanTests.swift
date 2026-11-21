import NotchCore
import XCTest

/// Settings' Turn FERB off until I open it again (APP-1110): what it asks, what it tells launchd, what it reads back.
final class TurnOffPlanTests: XCTestCase {
    // decision: APP-1110, APP-1111
    func testItAsksOnlyWhileATaskRuns() {
        XCTAssertNil(TurnOffPlan.question(running: 0), "nothing runs: FERB turns off at once")
        XCTAssertEqual(TurnOffPlan.question(running: 1), "A task is running. Stop it and turn FERB off?")
        XCTAssertEqual(TurnOffPlan.question(running: 3), "3 tasks are running. Stop them and turn FERB off?")
    }

    // decision: APP-1110, APP-1111, DMN-619
    func testItStopsFERBsOwnJobAndReadsWhetherItStillRuns() {
        XCTAssertEqual(TurnOffPlan.stop(uid: 501), ["kill", "SIGTERM", "gui/501/com.ferb.daemon"])
        // the daemon reads the same file name in FERB's home (ferb-device install.rs turned_off_marker)
        XCTAssertEqual(TurnOffPlan.offFile, "service.off")
        XCTAssertEqual(TurnOffPlan.pid("gui/501/com.ferb.daemon = {\n\tstate = running\n\tpid = 4242\n"), 4242)
        XCTAssertNil(TurnOffPlan.pid("gui/501/com.ferb.daemon = {\n\tstate = not running\n\tlast exit code = 0\n"))
        XCTAssertNil(TurnOffPlan.pid(""))
    }
}
