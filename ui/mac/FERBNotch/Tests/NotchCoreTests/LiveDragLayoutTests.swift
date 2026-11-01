import XCTest
import AppKit
import SwiftUI
@testable import FERBNotch

final class LiveDragLayoutTests: XCTestCase {
    func testOrientationUsesNearestEdgeEvenWhenPreviousSideHasEnoughRoom() {
        let screen = CGRect(x:0,y:0,width:1440,height:900)
        let cases: [(CGRect, FERBScreenEdge)] = [
            (CGRect(x:650,y:700,width:58,height:30), .top),
            (CGRect(x:650,y:170,width:58,height:30), .bottom),
            (CGRect(x:170,y:430,width:58,height:30), .left),
            (CGRect(x:1210,y:430,width:58,height:30), .right)]
        for (pill, edge) in cases {
            for previous in [FERBScreenEdge.left,.right,.top,.bottom] {
                XCTAssertEqual(LiveDragLayout.resolve(pill:pill,screen:screen,
                    preferred:CGSize(width:440,height:460),previous:previous).edge,edge)
            }
        }
    }

    func testStateMotionTracksDirectlyAndOnlyAnimatesStateChanges() {
        let first = CGRect(x:50,y:100,width:440,height:460)
        var motion = DragChatMotion(card:first,edge:.left)
        let moved = first.offsetBy(dx:30,dy:20)
        XCTAssertEqual(motion.update(target:moved,edge:.left,delta:CGPoint(x:30,y:20),at:1),moved)
        XCTAssertFalse(motion.isAnimating)
        let opposite = moved.offsetBy(dx:-400,dy:0)
        XCTAssertEqual(motion.update(target:opposite,edge:.right,delta:.zero,at:2),moved)
        let midway = motion.sample(at:2.09)
        XCTAssertLessThan(midway.minX,moved.minX)
        XCTAssertGreaterThan(midway.minX,opposite.minX)
        XCTAssertEqual(motion.update(target:moved,edge:.left,delta:.zero,at:2.09),midway)
        XCTAssertEqual(motion.sample(at:3),moved)
        XCTAssertFalse(motion.isAnimating)
        let next = moved.offsetBy(dx:10,dy:0)
        XCTAssertEqual(motion.update(target:next,edge:.left,delta:CGPoint(x:10,y:0),at:3),next)
    }

    func testInterruptedTransitionBeginsAtFittedVisibleSurface() {
        let initial = CGRect(x:50,y:100,width:440,height:460)
        var motion = DragChatMotion(card:initial,edge:.left)
        _ = motion.update(target:initial.offsetBy(dx:-400,dy:0),edge:.right,delta:.zero,at:0)
        _ = motion.sample(at:0.05)
        let visible = CGRect(x:0,y:100,width:500,height:460)
        XCTAssertEqual(motion.update(target:initial,edge:.top,delta:.zero,at:0.05,visibleCard:visible),visible)
        XCTAssertEqual(motion.sample(at:1),initial)
    }

    func testStateShiftAcceleratesGentlyThenSettlesWithoutBounce() {
        let initial = CGRect(x:50,y:100,width:440,height:460)
        let destination = initial.offsetBy(dx:400,dy:200)
        var motion = DragChatMotion(card:initial,edge:.left)
        _ = motion.update(target:destination,edge:.top,delta:.zero,at:0)
        let firstFrame = motion.sample(at:1.0/120)
        XCTAssertLessThan(firstFrame.minX-initial.minX, 8, "no abrupt high-velocity launch")
        let nextFrame = motion.sample(at:2.0/120)
        XCTAssertGreaterThan(nextFrame.minX-firstFrame.minX, firstFrame.minX-initial.minX)
        var previous = nextFrame
        for time in stride(from:3.0/120,through:1.0,by:1.0/120) {
            let frame = motion.sample(at:time)
            XCTAssertGreaterThanOrEqual(frame.minX,previous.minX)
            XCTAssertLessThanOrEqual(frame.minX,destination.minX)
            XCTAssertLessThanOrEqual(frame.minY,destination.minY)
            previous = frame
        }
        XCTAssertEqual(motion.sample(at:1),destination)
        XCTAssertFalse(motion.isAnimating)
    }

    func testReversalPreservesMotionVelocityAtVisibleGeometry() {
        let initial = CGRect(x:50,y:100,width:440,height:460)
        var motion = DragChatMotion(card:initial,edge:.left)
        _ = motion.update(target:initial.offsetBy(dx:400,dy:0),edge:.top,delta:.zero,at:0)
        let before = motion.sample(at:0.07999)
        let current = motion.sample(at:0.08)
        let visible = current.offsetBy(dx:10,dy:0)
        XCTAssertEqual(motion.update(target:initial,edge:.left,delta:.zero,at:0.08,visibleCard:visible),visible)
        let after = motion.sample(at:0.08001)
        XCTAssertEqual((after.minX-visible.minX)/0.00001,
                       (current.minX-before.minX)/0.00001,accuracy:2)
        XCTAssertEqual(motion.sample(at:1),initial)
    }

    func testReducedMotionCancelsStateShiftAndTracksImmediately() {
        let initial = CGRect(x:50,y:100,width:440,height:460)
        var motion = DragChatMotion(card:initial,edge:.left)
        let destination = initial.offsetBy(dx:400,dy:0)
        _ = motion.update(target:destination,edge:.right,delta:.zero,at:0)
        let moved = destination.offsetBy(dx:12,dy:9)
        XCTAssertEqual(motion.update(target:moved,edge:.right,delta:CGPoint(x:12,y:9),
                                     at:0.05,reduceMotion:true),moved)
        XCTAssertFalse(motion.isAnimating)
    }

    func testNearestEdgePreservesStateOnBisectorRegardlessOfGrabOffset() {
        let screen = CGRect(x:0,y:0,width:1000,height:1000)
        let pill = CGRect(x:171,y:185,width:58,height:30)
        XCTAssertEqual(LiveDragLayout.nearestEdge(pill:pill,screen:screen,previous:.left),.left)
        XCTAssertEqual(LiveDragLayout.nearestEdge(pill:pill,screen:screen,previous:.bottom),.bottom)
        XCTAssertEqual(LiveDragLayout.resolve(pill:pill,screen:screen,preferred:CGSize(width:440,height:460),previous:.left).edge,
            LiveDragLayout.nearestEdge(pill:pill,screen:screen,previous:.left))
    }

    func testChatChangesSidesWithoutLeavingDisplayAndStaysAttached() {
        let screen = CGRect(x: -1280, y: 0, width: 1280, height: 800)
        var edge = FERBScreenEdge.left
        for x in stride(from: -1270.0, through: -60.0, by: 20) {
            let pill = CGRect(x: x, y: 350, width: 58, height: 30)
            let result = LiveDragLayout.resolve(pill: pill, screen: screen, preferred: CGSize(width: 600, height: 700), previous: edge)
            edge = result.edge
            XCTAssertTrue(screen.contains(result.card))
            XCTAssertTrue(result.card.intersects(pill))
        }
        XCTAssertEqual(edge, .right)
    }
    func testAllEdgesAndSmallScreenKeepCardVisible() {
        let screen = CGRect(x: 0, y: 0, width: 800, height: 600)
        for pill in [CGRect(x: 350,y: 0,width:58,height:30), CGRect(x:350,y:570,width:58,height:30), CGRect(x:0,y:250,width:58,height:30), CGRect(x:742,y:250,width:58,height:30)] {
            let result = LiveDragLayout.resolve(pill: pill, screen: screen, preferred: CGSize(width: 1400,height:1000), previous:.left)
            XCTAssertTrue(screen.contains(result.card))
            XCTAssertTrue(result.card.intersects(pill))
        }
    }
    func testIntermediateFramesStayInsideDisplayAndGrabPointDoesNotChange() {
        let screen = CGRect(x:0,y:0,width:1440,height:900)
        let start = CGRect(x:0,y:300,width:58,height:30)
        let grabbed = CGPoint(x:17,y:311)
        let pointer = CGPoint(x:1300,y:600)
        let pill = PanelLayout.draggedFrame(start:start,grabbedAt:grabbed,pointer:pointer)
        XCTAssertEqual(pointer.x-pill.minX,17)
        XCTAssertEqual(pointer.y-pill.minY,11)
        let left = LiveDragLayout.resolve(pill:start,screen:screen,preferred:CGSize(width:600,height:700),previous:.left).card
        let right = LiveDragLayout.resolve(pill:pill,screen:screen,preferred:CGSize(width:600,height:700),previous:.right).card
        for t in stride(from:0.0,through:1.0,by:0.05) {
            XCTAssertTrue(screen.contains(LiveDragLayout.interpolate(left,right,progress:t)))
        }
    }
    func testRoundedSurfacesTouchAtDisplayCorners() {
        let screen = CGRect(x:0,y:0,width:1200,height:800)
        for pill in [CGRect(x:0,y:0,width:58,height:30),CGRect(x:0,y:770,width:58,height:30),
                     CGRect(x:1142,y:0,width:58,height:30),CGRect(x:1142,y:770,width:58,height:30)] {
            let card = LiveDragLayout.resolve(pill:pill,screen:screen,preferred:CGSize(width:440,height:460),previous:.left).card
            let cardPath = NotchSurfaceShape().path(in:card).cgPath
            let pillPath = CGPath(roundedRect:pill,cornerWidth:15,cornerHeight:15,transform:nil)
            var contact = false
            for x in stride(from:pill.minX,through:pill.maxX,by:1) {
                for y in stride(from:pill.minY,through:pill.maxY,by:1) {
                    let point = CGPoint(x:x,y:y)
                    if pillPath.contains(point) && cardPath.contains(point) { contact = true }
                }
            }
            XCTAssertTrue(contact, "rounded surfaces must overlap, not just bounding rectangles")
        }
    }
    func testRoundedContactDuringFastCrossingAndReversal() {
        let screen = CGRect(x:0,y:0,width:900,height:600)
        var card = CGRect(x:50,y:0,width:440,height:500)
        var previous = CGRect(x:0,y:0,width:58,height:30)
        var edge = FERBScreenEdge.left
        for x in [0.0,400,842,700,200,-10,0,842,400,0] {
            let pill = CGRect(x:x,y:0,width:58,height:30)
            card = card.offsetBy(dx:pill.minX-previous.minX,dy:0)
            let target = LiveDragLayout.resolve(pill:pill,screen:screen,preferred:CGSize(width:440,height:500),previous:edge)
            edge = target.edge
            for t in [0.0,0.1,0.5,0.9,1.0] {
                let intermediate = LiveDragLayout.connectedCard(LiveDragLayout.interpolate(card,target.card,progress:t),pill:pill,screen:screen)
                XCTAssertTrue(screen.contains(intermediate))
                let shape = NotchSurfaceShape().path(in:intermediate).cgPath
                let pillPath = CGPath(roundedRect:pill,cornerWidth:15,cornerHeight:15,transform:nil)
                var touches = false
                for px in stride(from:pill.minX,through:pill.maxX,by:2) {
                    for py in stride(from:pill.minY,through:pill.maxY,by:2) {
                        let point = CGPoint(x:px,y:py)
                        if shape.contains(point) && pillPath.contains(point) { touches = true }
                    }
                }
                XCTAssertTrue(touches)
            }
            card = target.card
            previous = pill
        }
    }

}
