#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import AVFAudio

/// A separate startup experiment. Uses the same sound cue as the intro; never installs anything.
@MainActor
final class PlaygroundArrival {
    private var panel: NSPanel?
    func show(seconds: Double) {
        hide()
        guard let screen = NSScreen.main else { return }
        let panel = ArrivalPanel(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel],
                                 backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 4)
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isReleasedWhenClosed = false
        panel.contentView = NSHostingView(rootView: ArrivalReveal(start: Date(), duration: max(0.1, seconds), envelope: ArrivalEnvelope.load(), study: StartupStudy.selected))
        panel.orderFrontRegardless()
        self.panel = panel
    }
    func hide() {
        panel?.orderOut(nil)
        panel?.close()
        panel = nil
    }
}

private final class ArrivalPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

private struct ArrivalReveal: View {
    let start: Date
    let duration: Double
    let envelope: ArrivalEnvelope
    let study: StartupStudy
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let logo = NSWorkspace.shared.icon(forFile: Bundle.main.bundlePath)

    var body: some View {
        TimelineView(.animation) { context in
            ArrivalFrame(elapsed: context.date.timeIntervalSince(start), duration: duration,
                         envelope: envelope, study: study, logo: logo, reduceMotion: reduceMotion)
        }
        .accessibilityHidden(true)
    }
}

/// The exact stage frame also renders offline for motion review without capturing the desktop.
private struct ArrivalFrame: View {
    let elapsed: Double
    let duration: Double
    let envelope: ArrivalEnvelope
    let study: StartupStudy
    let logo: NSImage
    let reduceMotion: Bool
    var body: some View {
        let progress = min(1, max(0, elapsed / duration))
        let revealStart = study.usesGravityTiming ? 0.78 : 0.62
        let reveal = min(1, max(0, (progress - revealStart) / (1 - revealStart)))
        let ease = reveal * reveal * reveal
        let appear = min(1, elapsed / 0.4)
        let energy = envelope.value(at: progress)
        let presence = appear * (1 - min(1, reveal * 3))
        let fieldOpacity = study.usesGravityTiming
            ? smooth(progress / 0.12) * (1 - smooth((progress - 0.72) / 0.09))
            : min(1, elapsed / 0.8) * presence
        GeometryReader { geometry in
            let expansion = 112 + max(geometry.size.width, geometry.size.height) * 2.3 * ease
            ZStack {
                ZStack {
                    Color(red: 0.018, green: 0.019, blue: 0.022)
                    if !reduceMotion {
                        RoundedRectangle(cornerRadius: expansion * 22 / 96, style: .continuous)
                            .fill(.black)
                            .frame(width: expansion, height: expansion)
                            .opacity(reveal > 0 ? 1 : 0)
                            .blendMode(.destinationOut)
                    }
                }
                .compositingGroup()
                .opacity(reduceMotion ? 1 - reveal : 1)
                ZStack {
                    if study != .still {
                        StartupStudyField(study: study, time: reduceMotion ? 0 : elapsed,
                                          progress: reduceMotion ? 0.4 : progress, energy: reduceMotion ? 0 : energy,
                                          resonance: !reduceMotion && study == .gravityDense
                                            ? envelope.ringResponses(at: progress) : [])
                            .mask {
                                RadialGradient(colors: [.white, .white.opacity(0.9), .clear],
                                               center: .center, startRadius: 60,
                                               endRadius: max(geometry.size.width, geometry.size.height) * 0.65)
                            }
                            .opacity(fieldOpacity)
                    }
                    Image(nsImage: logo)
                        .resizable().interpolation(.high)
                        .frame(width: 150, height: 150)
                        .shadow(color: .black.opacity(study == .gravityDense ? 0.18 : 0.8),
                                radius: study == .gravityDense ? 4 : 24)
                        .opacity(presence)
                }
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
            .opacity(appear)
        }
    }
    private func smooth(_ value: Double) -> Double {
        let t = min(1, max(0, value))
        return t * t * (3 - 2 * t)
    }

}
/// Read only the bundled cue, once per entrance. No microphone or live audio capture.
private struct ArrivalEnvelope {
    let samples: [Double]
    var resonantSamples: [Double] = []
    var seconds: Double = 1

    /// Swells excite a damped response, arriving at outer rings later than the material beside the mark.
    func ringResponses(at progress: Double) -> [Double] {
        (0...64).map { ring in
            let delayed = progress - Double(ring) / 64 * 0.85 / seconds
            guard delayed >= 0, !resonantSamples.isEmpty else { return 0 }
            let position = min(1, delayed) * Double(resonantSamples.count - 1)
            let lower = Int(position), upper = min(resonantSamples.count - 1, lower + 1)
            return resonantSamples[lower] + (resonantSamples[upper] - resonantSamples[lower])
                * (position - Double(lower))
        }
    }

    func value(at progress: Double) -> Double {
        guard !samples.isEmpty else { return 0 }
        let position = min(1, max(0, progress)) * Double(samples.count - 1)
        let lower = Int(position)
        let upper = min(samples.count - 1, lower + 1)
        return samples[lower] + (samples[upper] - samples[lower]) * (position - Double(lower))
    }

    static func load() -> ArrivalEnvelope {
        guard let url = FerbSoundFiles.files(for: .firstLaunch, subtle: false).first,
              let file = try? AVAudioFile(forReading: url),
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat,
                                            frameCapacity: AVAudioFrameCount(file.length)),
              (try? file.read(into: buffer)) != nil,
              let channels = buffer.floatChannelData else { return ArrivalEnvelope(samples: []) }
        let count = Int(buffer.frameLength)
        let strideSize = max(1, Int(file.processingFormat.sampleRate * 0.025))
        var raw: [Double] = []
        for start in stride(from: 0, to: count, by: strideSize) {
            let end = min(count, start + strideSize)
            var squares = 0.0
            for channel in 0..<Int(buffer.format.channelCount) {
                for index in start..<end { squares += Double(channels[channel][index]) * Double(channels[channel][index]) }
            }
            raw.append(sqrt(squares / Double((end - start) * Int(buffer.format.channelCount))))
        }
        let maximum = max(0.0001, raw.max() ?? 0)
        let smooth = raw.indices.map { index -> Double in
            let range = max(0, index - 2)...min(raw.count - 1, index + 2)
            let level = range.reduce(0.0) { $0 + raw[$1] } / Double(range.count)
            return sqrt(level / maximum)
        }
        // Analyze the bundled file once. A damped spring follows its envelope; its velocity supplies
        // the signed elastic response, so quiet passages settle naturally instead of looping a pulse.
        let step = Double(strideSize) / file.processingFormat.sampleRate
        var position = 0.0, velocity = 0.0
        var response: [Double] = []
        for level in smooth {
            // The first sample is rest at t = 0, so a wave enters outer rings continuously.
            response.append(velocity)
            for _ in 0..<4 {
                let dt = step / 4
                velocity += (70 * (level - position) - 9 * velocity) * dt
                position += velocity * dt
            }
        }
        let responsePeak = max(0.0001, response.map { abs($0) }.max() ?? 0)
        return ArrivalEnvelope(samples: smooth, resonantSamples: response.map { $0 / responsePeak },
                               seconds: Double(count) / file.processingFormat.sampleRate)
    }
}
#endif
