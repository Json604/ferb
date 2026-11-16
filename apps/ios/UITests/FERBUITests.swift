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
        app.launchArguments = ["-FERBUITestUnlock", "YES", "-FERBUITestFresh", "YES"] + more
        app.launch()
        return app
    }

    // decision: PHN-8, PHN-505
    func testLaunchIsLockedThenOnePromptOpensThePairingPage() throws {
        let app = launch()
        XCTAssertTrue(app.staticTexts["FERB is locked"].waitForExistence(timeout: 30), "nothing of FERB's before the lock")
        XCTAssertFalse(app.staticTexts["Use FERB from your phone"].exists)
        app.buttons["unlock"].tap()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 30), "a fresh install pairs first")
    }

    // decision: PHN-442, PHN-564
    func testATypedCodeTheMacNeverMadeIsRefusedInPlainWords() throws {
        let app = launch()
        XCTAssertTrue(app.buttons["unlock"].waitForExistence(timeout: 30))
        app.buttons["unlock"].tap()
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
    // decision: PHN-564
    func testTheScannerWithoutACameraOffersTheTypedCode() throws {
        let app = launch()
        XCTAssertTrue(app.buttons["unlock"].waitForExistence(timeout: 30))
        app.buttons["unlock"].tap()
        let scan = app.buttons["Scan the code"]
        XCTAssertTrue(scan.waitForExistence(timeout: 30))
        scan.tap()
        XCTAssertTrue(app.staticTexts["This iPhone has no camera FERB can use here. Type the code instead."].waitForExistence(timeout: 10))
    }

    /// Leaving FERB for the Home Screen and coming back within five minutes doesn't lock it (PHN-15).
    // decision: PHN-15, PHN-565
    func testComingBackSoonStaysOpen() throws {
        let app = launch()
        XCTAssertTrue(app.buttons["unlock"].waitForExistence(timeout: 30))
        app.buttons["unlock"].tap()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 30))
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(app.staticTexts["Use FERB from your phone"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["FERB is locked"].exists)
    }
}
