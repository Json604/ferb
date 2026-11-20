import AVFoundation
import CoreImage
import Foundation
import NotchCore

/// The Mac's camera for the phone's Show me (APP-983), in FERB.app's own process so the permission is FERB's
/// (NSCameraUsageDescription; x2 experiment 8). It runs only between the daemon's `insomnia.camera {on: true}` and
/// `{on: false}` (the daemon turns it off when the phone stops pulling). About every 0.5 s one JPEG, about 640 points
/// wide at quality 0.6, goes to the daemon (`insomnia.camera_frame`), which keeps the latest in memory only. Nothing is
/// ever written to disk or logged here; a frame lives only until it is sent.
final class InsomniaCamera: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, @unchecked Sendable {
    static let width: CGFloat = 640
    static let quality: CGFloat = 0.6
    static let interval: TimeInterval = 0.5

    /// FERB.app's camera permission, as `insomnia.report`'s shield event names it.
    static var access: String {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: return "allowed"
        case .notDetermined: return "not_asked"
        default: return "denied"
        }
    }

    /// Asked only when the owner turns the mode on from the Mac (InsomniaModel.startInsomnia); tests stand in their own.
    nonisolated(unsafe) static var askAccessIfNeeded: @Sendable () async -> Void = {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined else { return }
        _ = await AVCaptureDevice.requestAccess(for: .video)
    }

    private let rpc: DaemonRPCTransport
    /// "on", "off", "denied" or "failed", on the main actor.
    private let onState: @MainActor (String) -> Void
    private let queue = DispatchQueue(label: "com.ferb.FERB.insomnia-camera")
    private let context = CIContext(options: [.cacheIntermediates: false])
    // On `queue` only:
    private var session: AVCaptureSession?
    private var seq = 0
    private var lastSent: TimeInterval = 0
    private var sending = false

    init(rpc: DaemonRPCTransport, onState: @escaping @MainActor (String) -> Void) {
        self.rpc = rpc
        self.onState = onState
    }

    func start() {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            // never asked at the moment of an alert: not yet allowed reads as refused
            tell("denied")
            return
        }
        queue.async { [self] in
            guard session == nil else { return }
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device) else { return tell("failed") }
            let capture = AVCaptureSession()
            capture.sessionPreset = .vga640x480
            let output = AVCaptureVideoDataOutput()
            output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
            output.alwaysDiscardsLateVideoFrames = true
            output.setSampleBufferDelegate(self, queue: queue)
            guard capture.canAddInput(input), capture.canAddOutput(output) else { return tell("failed") }
            capture.addInput(input)
            capture.addOutput(output)
            session = capture
            lastSent = 0
            capture.startRunning()
            tell(capture.isRunning ? "on" : "failed")
            if !capture.isRunning { session = nil }
        }
    }

    func stop() {
        queue.async { [self] in
            guard let capture = session else { return }
            session = nil
            capture.stopRunning()
            tell("off")
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        let time = ProcessInfo.processInfo.systemUptime
        guard session != nil, !sending, time - lastSent >= Self.interval,
              let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let image = CIImage(cvPixelBuffer: pixels)
        let scale = min(1, Self.width / max(1, image.extent.width))
        let small = image.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        guard let jpeg = context.jpegRepresentation(
            of: small, colorSpace: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            options: [CIImageRepresentationOption(rawValue: kCGImageDestinationLossyCompressionQuality as String): Self.quality])
        else { return }
        lastSent = time
        seq += 1
        sending = true
        let params: [String: Any] = ["seq": seq, "jpeg": jpeg.base64EncodedString()]
        Task { [rpc, queue, weak self] in
            _ = try? await rpc.call("insomnia.camera_frame", params: params)
            queue.async { self?.sending = false }
        }
    }

    private func tell(_ state: String) {
        Task { @MainActor [onState] in onState(state) }
    }
}
