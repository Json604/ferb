import XCTest
import AppKit
@testable import FERBNotch

final class DragCanvasCursorTests: XCTestCase {
    @MainActor func testNonKeyFirstHoverAndChangedBorderUseTrackingWithoutClick() {
        let model = NotchModel(preferences: scratchDefaults())
        model.expand()
        let window = NSWindow(contentRect: CGRect(x: 0,y: 0,width: 700,height: 700), styleMask: .borderless, backing: .buffered, defer: true)
        let backdrop = NotchBackdropView(frame: CGRect(x:0,y:0,width:700,height:700))
        window.contentView = backdrop
        let resize = PanelResizeOverlay(model:model)
        backdrop.addSubview(resize)
        resize.frame = CGRect(x:20,y:20,width:440,height:500)
        resize.updateTrackingAreas()
        XCTAssertFalse(window.isKeyWindow)
        XCTAssertTrue(resize.trackingAreas.contains { $0.options.contains(.activeAlways) && $0.options.contains(.cursorUpdate) })
        var applied: [NSCursor] = []
        resize.applyCursor = { applied.append($0) }
        func hover(_ point: CGPoint) {
            let pointInWindow = resize.convert(point,to:nil)
            let event = NSEvent.mouseEvent(with:.mouseMoved,location:pointInWindow,modifierFlags:[],timestamp:0,windowNumber:window.windowNumber,context:nil,eventNumber:0,clickCount:0,pressure:0)!
            resize.mouseMoved(with:event)
        }
        hover(CGPoint(x:439,y:100))
        XCTAssertEqual(applied.count,1)
        resize.frame.size.width = 600
        hover(CGPoint(x:599,y:100))
        XCTAssertEqual(applied.count,2)
        resize.dragExclusionRect = CGRect(x:600,y:100,width:30,height:80)
        hover(CGPoint(x:599,y:100))
        XCTAssertTrue(applied.last === NSCursor.arrow)
        let count = applied.count
        hover(CGPoint(x:250,y:250))
        XCTAssertEqual(applied.count,count, "Interior must not override text cursors")
    }

    @MainActor func testNativeDragCanvasKeepsGrabbedControlFixedWhileOnlyCardMoves() {
        let screen = CGRect(x:-1400,y:0,width:1400,height:900)
        let window = NSWindow(contentRect: CGRect(x:-1000,y:200,width:650,height:500),styleMask:.borderless,backing:.buffered,defer:true)
        let presentation = NotchPresentation()
        window.contentView = presentation.backdrop
        let control = NSView(frame:.zero)
        presentation.backdrop.addSubview(control)
        var pill = CGRect(x:-720,y:430,width:58,height:30)
        for i in 0...10 {
            let card = CGRect(x:-1300+CGFloat(i)*60,y:150,width:600,height:650)
            DragSurfaceCanvas.place(window:window,presentation:presentation,control:control,pill:pill,card:card,canvas:screen,bodyVisible:true)
            XCTAssertEqual(window.frame,screen)
            let actual = window.convertToScreen(control.convert(control.bounds,to:nil))
            XCTAssertEqual(actual,pill)
        }
        pill = pill.offsetBy(dx:120,dy:80)
        DragSurfaceCanvas.place(window:window,presentation:presentation,control:control,pill:pill,card:CGRect(x:-700,y:150,width:600,height:650),canvas:screen,bodyVisible:true)
        XCTAssertEqual(window.convertToScreen(control.convert(control.bounds,to:nil)),pill)
        XCTAssertEqual(window.frame,screen)
        let compact = CGRect(x:-700,y:150,width:600,height:650).union(pill)
        DragSurfaceCanvas.place(window:window,presentation:presentation,control:control,pill:pill,card:CGRect(x:-700,y:150,width:600,height:650),canvas:compact,bodyVisible:true)
        XCTAssertEqual(window.frame,compact)
        XCTAssertEqual(window.convertToScreen(control.convert(control.bounds,to:nil)),pill)
    }
}
