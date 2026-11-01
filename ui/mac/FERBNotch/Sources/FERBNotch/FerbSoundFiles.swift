import AVFAudio
import Accelerate
import NotchCore

/// The decided family's files (rendered by demo/sound-family/render.py, shipped in Resources/Sounds): dry mono
/// sources named `ferb-<sound>-<standard|subtle>[-v<n>].wav`, the signature and orb sounds without an intensity,
/// the room as a stereo impulse response, and the first-launch clip.
enum FerbSoundFiles {
    static var directory: URL? { Bundle.module.url(forResource: "Sounds", withExtension: nil) }
    static var roomImpulse: URL? { file("ferb-room-ir.wav") }

    static func baseName(_ s: FerbSound) -> String {
        switch s {
        case .firstLaunch: return "first-launch"
        case .intro: return "logo"
        case .summon: return "summon"
        case .listeningStart: return "listen-start"
        case .listeningEnd: return "listen-end"
        case .done: return "done"
        case .needsYou: return "needs-you"
        case .paused: return "paused"
        case .error: return "error"
        case .notification: return "notification"
        case .orbDrip: return "orb-drip"
        case .orbHop: return "orb-hop"
        case .orbMelt: return "orb-melt"
        }
    }

    /// Every file that can play for a sound at an intensity: its variants (one is picked at random), else its one
    /// file. A sound with no Subtle render (the logo, the orb, the first-launch clip) plays its only file at Subtle.
    static func files(for s: FerbSound, subtle: Bool) -> [URL] {
        let base = "ferb-" + baseName(s)
        for intensity in subtle ? ["subtle", "standard"] : ["standard"] {
            let variants = (1...3).compactMap { file("\(base)-\(intensity)-v\($0).wav") }
            if !variants.isEmpty { return variants }
            if let one = file("\(base)-\(intensity).wav") { return [one] }
        }
        return file("\(base).wav").map { [$0] } ?? []
    }

    /// The finished mixes that already carry their own space: played as they are, never through the room.
    static func isFinishedMix(_ s: FerbSound) -> Bool { s == .firstLaunch }

    /// render.py's Crystal room send (0.13), lower for the error (×0.6) and at Subtle (×0.8).
    static func roomSend(_ s: FerbSound, subtle: Bool) -> Float {
        0.13 * (s == .error ? 0.6 : 1) * (subtle ? 0.8 : 1)
    }

    private static func file(_ name: String) -> URL? {
        guard let url = directory?.appendingPathComponent(name),
              FileManager.default.fileExists(atPath: url.path) else { return nil }
        return url
    }
}

/// One loaded sound: its channels (one for a dry source, two for a finished mix) and, for a dry source, its room
/// (two channels at the sound's send), a convolution of the source with the room's impulse response, computed once.
struct FerbSoundData {
    static let sampleRate = 48_000.0
    let channels: [[Float]]
    let room: [[Float]]?
}

/// Loads and caches each file's samples and its room; all the signal work is offline, on short buffers.
@MainActor
final class FerbSoundLibrary {
    private var cache: [URL: FerbSoundData] = [:]
    private var impulse: [[Float]]?

    /// `roomSend` nil: the file plays as it is. A file's send never changes (its name fixes sound and intensity).
    func data(for url: URL, roomSend: Float?) -> FerbSoundData? {
        if let hit = cache[url], hit.room != nil || roomSend == nil { return hit }
        guard let channels = Self.read(url) else { return nil }
        var room: [[Float]]?
        if let send = roomSend, channels.count == 1, let ir = roomImpulse(), ir.count == 2 {
            room = Self.room(for: channels[0], impulse: ir, send: send,
                             tail: url.lastPathComponent == "ferb-logo.wav" ? 0.6 : 0.3)
        }
        let loaded = FerbSoundData(channels: channels, room: room)
        cache[url] = loaded
        return loaded
    }

    /// The room as render.py makes its stem: the convolution at the send over the source plus `tail` seconds, cut
    /// where the mix (the source on both sides plus the room) falls 60 dB under its peak (plus 4 ms), then faded.
    static func room(for dry: [Float], impulse: [[Float]], send: Float, tail: Double) -> [[Float]] {
        let length = dry.count + Int(tail * FerbSoundData.sampleRate)
        let wet = impulse.map { h -> [Float] in
            let full = convolve(dry, h).prefix(length).map { $0 * send }
            return full + [Float](repeating: 0, count: length - full.count)
        }
        var lastAbove = length - 1
        var peak: Float = 0
        var envelope = [Float](repeating: 0, count: length)
        for i in 0..<length {
            let d = i < dry.count ? dry[i] : 0
            envelope[i] = max(abs(d + wet[0][i]), abs(d + wet[1][i]))
            peak = max(peak, envelope[i])
        }
        if let i = envelope.lastIndex(where: { $0 > peak * 0.001 }) { lastAbove = i }
        let end = min(length, lastAbove + Int(0.004 * FerbSoundData.sampleRate))
        return wet.map { fadeOut(Array($0.prefix(end))) }
    }

    private func roomImpulse() -> [[Float]]? {
        if impulse == nil, let url = FerbSoundFiles.roomImpulse { impulse = Self.read(url) }
        return impulse
    }

    /// The file's channels as 32-bit float at 48 kHz (every file render.py writes); nil for anything else.
    static func read(_ url: URL) -> [[Float]]? {
        guard let file = try? AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false),
              file.processingFormat.sampleRate == FerbSoundData.sampleRate,
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)),
              (try? file.read(into: buffer)) != nil, let data = buffer.floatChannelData else { return nil }
        let frames = Int(buffer.frameLength)
        return (0..<Int(buffer.format.channelCount)).map { Array(UnsafeBufferPointer(start: data[$0], count: frames)) }
    }

    /// The room stem's end, faded as render.py fades it (a cos² over the last 14 %, at least 15 ms), ending on zero.
    static func fadeOut(_ x: [Float]) -> [Float] {
        guard !x.isEmpty else { return x }
        var y = x
        let count = min(y.count, max(Int(0.015 * FerbSoundData.sampleRate), Int(0.14 * Double(y.count))))
        for i in 0..<count {
            let w = cos(0.5 * Float.pi * Float(i) / Float(max(count - 1, 1)))
            y[y.count - count + i] *= w * w
        }
        y[y.count - 1] = 0
        return y
    }

    /// A gentle (6 dB per octave) low-pass, for distance from the notch.
    static func lowPass(_ x: [Float], cutoff: Double) -> [Float] {
        let a = Float(exp(-2 * Double.pi * cutoff / FerbSoundData.sampleRate))
        var y = x
        var state: Float = 0
        for i in y.indices {
            state = (1 - a) * y[i] + a * state
            y[i] = state
        }
        return y
    }

    /// Linear convolution through vDSP's real FFT (one forward per signal, one inverse).
    static func convolve(_ x: [Float], _ h: [Float]) -> [Float] {
        guard !x.isEmpty, !h.isEmpty else { return [] }
        let count = x.count + h.count - 1
        let log2n = vDSP_Length(ceil(log2(Double(count))))
        let n = 1 << Int(log2n), half = n / 2
        guard let setup = vDSP_create_fftsetup(log2n, FFTRadix(kFFTRadix2)) else { return [] }
        defer { vDSP_destroy_fftsetup(setup) }
        func spectrum(_ s: [Float]) -> (re: [Float], im: [Float]) {
            var padded = s + [Float](repeating: 0, count: n - s.count)
            var re = [Float](repeating: 0, count: half), im = [Float](repeating: 0, count: half)
            re.withUnsafeMutableBufferPointer { r in
                im.withUnsafeMutableBufferPointer { i in
                    var split = DSPSplitComplex(realp: r.baseAddress!, imagp: i.baseAddress!)
                    padded.withUnsafeMutableBufferPointer { p in
                        p.baseAddress!.withMemoryRebound(to: DSPComplex.self, capacity: half) {
                            vDSP_ctoz($0, 2, &split, 1, vDSP_Length(half))
                        }
                    }
                    vDSP_fft_zrip(setup, &split, 1, log2n, FFTDirection(kFFTDirection_Forward))
                }
            }
            return (re, im)
        }
        var a = spectrum(x)
        var b = spectrum(h)
        // Packed format: element 0 holds the DC term (real) and the Nyquist term (imaginary), multiplied apart.
        let dc = a.re[0] * b.re[0], nyquist = a.im[0] * b.im[0]
        var out = [Float](repeating: 0, count: n)
        a.re.withUnsafeMutableBufferPointer { ar in
            a.im.withUnsafeMutableBufferPointer { ai in
                b.re.withUnsafeMutableBufferPointer { br in
                    b.im.withUnsafeMutableBufferPointer { bi in
                        var sa = DSPSplitComplex(realp: ar.baseAddress!, imagp: ai.baseAddress!)
                        var sb = DSPSplitComplex(realp: br.baseAddress!, imagp: bi.baseAddress!)
                        vDSP_zvmul(&sa, 1, &sb, 1, &sa, 1, vDSP_Length(half), 1)
                        ar[0] = dc
                        ai[0] = nyquist
                        vDSP_fft_zrip(setup, &sa, 1, log2n, FFTDirection(kFFTDirection_Inverse))
                        out.withUnsafeMutableBufferPointer { o in
                            o.baseAddress!.withMemoryRebound(to: DSPComplex.self, capacity: half) {
                                vDSP_ztoc(&sa, 1, $0, 2, vDSP_Length(half))
                            }
                        }
                    }
                }
            }
        }
        // Each forward real FFT scales by 2 and the inverse by n.
        var scale = 1 / Float(4 * n)
        vDSP_vsmul(out, 1, &scale, &out, 1, vDSP_Length(n))
        return Array(out.prefix(count))
    }
}
