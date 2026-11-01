import XCTest
import CoreGraphics
@testable import FERBNotch
@testable import NotchCore

/// The orb's keyframe timing against the output's latency, and its homecoming hook (fix round 1).
@MainActor
final class OrbControllerTests: XCTestCase {
    private let home = CGPoint(x: 720, y: 885)
    private let target = CGPoint(x: 200, y: 400)

    private func makeOrb(latency: Double) -> (OrbController, FakeOrbSurface, FakeSoundPlayer, ManualClock) {
        let surface = FakeOrbSurface(), clock = ManualClock(), sound = FakeSoundPlayer()
        sound.clock = clock
        sound.outputLatency = latency
        let orb = OrbController(surface: surface, sound: sound, clock: clock, home: { [home] in home },
                                reduceMotion: { false }, makeTracker: { FakeTracker() })
        return (orb, surface, sound, clock)
    }

    private func run(_ clock: ManualClock, seconds: Double, until done: () -> Bool = { false }) {
        let end = clock.now + seconds
        while clock.now < end && !done() { clock.advance(1.0 / 120) }
    }

    func testKeyframeSoundsLeadTheirKeyframesByTheOutputLatency() async {
        let (orb, surface, sound, clock) = makeOrb(latency: 0.1)
        let flight = Task { await orb.launch(to: target, instruction: nil) }
        await Task.yield()
        XCTAssertEqual(sound.played, [.orbDrip])
        run(clock, seconds: 0.08)
        XCTAssertEqual(surface.center, home)                 // held while the drip reaches the speakers
        XCTAssertEqual(surface.opacity, 0)
        run(clock, seconds: 0.1)
        XCTAssertNotEqual(surface.center, home)
        run(clock, seconds: 3, until: { surface.center == target })
        let landed = clock.now
        await flight.value
        let melt = try? XCTUnwrap(sound.times.first { $0.0 == .orbMelt }?.1)
        XCTAssertEqual(landed - (melt ?? 0), 0.1, accuracy: 1.0 / 60)   // the melt was issued ahead of the landing
        let hopAt = clock.now
        orb.hop()
        XCTAssertEqual(sound.played.last, .orbHop)
        XCTAssertEqual(sound.times.last?.1 ?? -1, hopAt, accuracy: 1e-9)
        run(clock, seconds: 0.08)
        XCTAssertEqual(surface.center, target)               // the lift waits for the hop to be heard
        run(clock, seconds: 0.1)
        XCTAssertGreaterThan(surface.center.y, target.y)
    }

    func testAHopDuringTheDeparturesHoldWaitsOneLeadFromItsOwnCall() async {
        let (hopping, hopSurface, _, hopClock) = makeOrb(latency: 0.1)
        let (still, stillSurface, _, stillClock) = makeOrb(latency: 0.1)
        let first = Task { await hopping.launch(to: target, instruction: nil) }
        let second = Task { await still.launch(to: target, instruction: nil) }
        await Task.yield()
        hopping.hop()                                        // at t=0, while the drip holds the departure
        run(hopClock, seconds: 0.08)
        run(stillClock, seconds: 0.08)
        XCTAssertEqual(hopSurface.center, stillSurface.center)   // not yet: the hop's sound is still on its way
        run(hopClock, seconds: 0.07)
        run(stillClock, seconds: 0.07)
        XCTAssertGreaterThan(hopSurface.center.y - stillSurface.center.y, 1)   // lifting at 0.15 s, not after 0.2 s
        hopping.tearDown()
        still.tearDown()
        await first.value
        await second.value
    }

    func testWithNoLatencyTheMeltPlaysAsTheOrbLands() async {
        let (orb, surface, sound, clock) = makeOrb(latency: 0)
        let flight = Task { await orb.launch(to: target, instruction: nil) }
        await Task.yield()
        run(clock, seconds: 3, until: { surface.center == target })
        await flight.value
        XCTAssertEqual(sound.times.first { $0.0 == .orbMelt }?.1 ?? -1, clock.now, accuracy: 1e-9)
    }

    func testTheHomecomingHookFiresOnlyWhenTheOrbMeltsBackIntoTheNotch() async {
        let (orb, surface, sound, clock) = makeOrb(latency: 0)
        var homecomings = 0
        orb.onHomeArrival = { homecomings += 1 }
        let out = Task { await orb.launch(to: target, instruction: nil) }
        await Task.yield()
        run(clock, seconds: 3, until: { surface.center == target })
        await out.value
        XCTAssertEqual(homecomings, 0)                       // arriving at a target isn't coming home
        let back = Task { await orb.flyHome() }
        await Task.yield()
        run(clock, seconds: 3)
        await back.value
        XCTAssertEqual(homecomings, 1)
        XCTAssertFalse(surface.visible)
        XCTAssertEqual(sound.played.filter { $0 == .orbMelt }.count, 2)
        let again = Task { await orb.launch(to: target, instruction: nil) }
        await Task.yield()
        run(clock, seconds: 0.1)
        orb.tearDown()                                       // a tear-down never counts as a homecoming
        await again.value
        XCTAssertEqual(homecomings, 1)
    }
}
