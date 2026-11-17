import XCTest

/// The app on the simulator, from the outside: what a person sees at launch. The simulator has no Mac to pair with,
/// so these stop where a Mac would be needed. `-FERBUITestUnlock` is honoured only by a Debug build on the simulator
/// (FerbAppUI Prompts): it stands in for the Face ID or passcode sheet, which a UI test cannot answer.
final class FERBUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(_ more: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-FERBUITestUnlock", "YES", "-FERBUITestFresh", "YES", "-FERBUITestFirstRunDone", "YES"] + more
        app.launch()
        return app
    }

    /// A fresh install asks for what FERB needs once, after the unlock (kb PHN-636); Not now still leads to pairing
    /// (PHN-639).
    // decision: PHN-635, PHN-636, PHN-639
    func testTheFirstRunAsksOnceAndNotNowStillPairs() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-FERBUITestUnlock", "YES", "-FERBUITestFresh", "YES"]
        app.launch()
        XCTAssertTrue(app.staticTexts["Before you start"].waitForExistence(timeout: 30), "the first run's asks after the unlock")
        app.buttons["Not now"].tap()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 10), "pairing with everything declined")
        // never again on a relaunch: FirstRunTests (a relaunch here would meet the simulator's own passcode sheet)
    }

    /// The screenshot shield on this iOS: the secure canvas is found and the shielded half is drawn where it belongs
    /// beside a plain half, taking touches through (kb PHN-597). Whether a screenshot is blank needs an iPhone (the live
    /// check): the simulator does not honour the mark.
    // decision: PHN-597
    func testTheShieldIsFoundAndDrawnInPlace() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-FERBUITestShield", "YES"]
        app.launch()
        let plain = app.staticTexts["shield-plain"], shielded = app.otherElements["shield-shielded"]
        XCTAssertTrue(plain.waitForExistence(timeout: 30))
        XCTAssertTrue(shielded.waitForExistence(timeout: 10))
        XCTAssertEqual(shielded.label, "shielded", "this iOS builds the secure canvas")
        XCTAssertEqual(shielded.frame.minY, plain.frame.maxY, accuracy: 1)
        XCTAssertEqual(shielded.frame.height, plain.frame.height, accuracy: 1)
        XCTAssertTrue(shielded.isHittable)
    }

    /// Opening FERB asks by itself: the stand-in prompt passes at once and the pairing page shows, nothing tapped.
    // decision: PHN-8, PHN-505, PHN-615
    func testLaunchAsksByItselfAndOpensThePairingPage() throws {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 30), "a fresh install pairs first")
    }

    /// A cancelled prompt leaves the lock page with its button and does not come back by itself; the button asks.
    // decision: PHN-8, PHN-505, PHN-615, PHN-616
    func testACancelledPromptWaitsForTheButton() throws {
        let app = launch(["-FERBUITestCancelPrompts", "1"])
        XCTAssertTrue(app.staticTexts["FERB is locked"].waitForExistence(timeout: 30), "nothing of FERB's before the lock")
        XCTAssertTrue(app.buttons["unlock"].waitForExistence(timeout: 10))
        // a second prompt by itself would pass at once and open the pairing page
        XCTAssertFalse(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 4), "no prompt again after a cancel")
        XCUIDevice.shared.orientation = .landscapeLeft
        XCUIDevice.shared.orientation = .portrait
        XCTAssertFalse(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 3), "none after a turn of the phone")
        app.buttons["unlock"].tap()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 30))
    }

    // decision: PHN-442, PHN-564
    func testATypedCodeTheMacNeverMadeIsRefusedInPlainWords() throws {
        let app = launch()
        let typeCode = app.buttons["Type code"]
        XCTAssertTrue(typeCode.waitForExistence(timeout: 30))
        typeCode.tap()
        let field = app.textFields["FERB-…"]
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        field.tap()
        field.typeText("FERB-not-a-code")
        app.buttons["Pair"].tap()
        XCTAssertTrue(app.staticTexts["That code didn't work. Check it on your Mac and try again."].waitForExistence(timeout: 30))
    }

    /// The scanner on a phone without a camera (the simulator) says so and offers the typed code.
    // decision: PHN-564, PHN-619
    func testTheScannerWithoutACameraOffersTheTypedCode() throws {
        let app = launch()
        let scan = app.buttons["Scan the code"]
        XCTAssertTrue(scan.waitForExistence(timeout: 30))
        scan.tap()
        XCTAssertTrue(app.staticTexts["This iPhone has no camera FERB can use here. Type the code instead."].waitForExistence(timeout: 10))
    }

    /// Leaving FERB for the Home Screen and coming back within five minutes doesn't lock it (PHN-15).
    // decision: PHN-15, PHN-597
    func testComingBackSoonStaysOpen() throws {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 30))
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["FERB is locked"].exists)
    }
}
