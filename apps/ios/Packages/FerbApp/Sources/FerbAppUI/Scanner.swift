import AVFoundation
import SwiftUI
import UIKit

/// The pairing code's reader (kb PHN-447's iPhone half, PHN-564): AVFoundation's own QR reader
/// (AVCaptureMetadataOutput, type .qr), the back camera, no other library and nothing recorded. Chosen over VisionKit's
/// DataScannerViewController because it is what PHN-447 named, needs no Neural Engine (VisionKit's scanner refuses
/// phones without one and the simulator), reads exactly one symbology, and hands the text over the moment it is seen
/// (PHN-135); the frames never leave the capture session. The code read is passed on once.
public struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    public init(found: @escaping (String) -> Void) {
        self.found = found
    }

    public func makeUIViewController(context: Context) -> ScannerController {
        let c = ScannerController()
        c.found = found
        return c
    }

    public func updateUIViewController(_ controller: ScannerController, context: Context) {}

    /// The camera FERB can use, or nil (the simulator has none).
    static var camera: AVCaptureDevice? { AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back) }
}

public final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var found: ((String) -> Void)?
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "com.ferb.phone.scanner")
    private var done = false

    public override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        // the picture fills the view and is cut at its edges (kb PHN-618): a preview layer drawn aspect-fill spills
        // past its own bounds otherwise; the frame's rounded well clips the view in turn
        view.clipsToBounds = true
        guard let camera = QRScanner.camera, let input = try? AVCaptureDeviceInput(device: camera),
              session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.masksToBounds = true
        preview.frame = view.bounds
        view.layer.addSublayer(preview)
        if (try? camera.lockForConfiguration()) != nil {
            if camera.isFocusModeSupported(.continuousAutoFocus) { camera.focusMode = .continuousAutoFocus }
            camera.unlockForConfiguration()
        }
    }

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        view.layer.sublayers?.forEach { $0.frame = view.bounds }
    }

    public override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        let s = session
        queue.async { if !s.isRunning { s.startRunning() } }
    }

    public override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let s = session
        queue.async { if s.isRunning { s.stopRunning() } }
    }

    public func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !done, let code = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first else { return }
        done = true
        let s = session
        queue.async { s.stopRunning() }
        found?(code)
    }
}

/// Where the camera stands for FERB.
enum CameraAccess {
    case allowed, notAsked, denied, none

    static var now: CameraAccess {
        guard QRScanner.camera != nil else { return .none }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: return .allowed
        case .notDetermined: return .notAsked
        default: return .denied
        }
    }

    /// iOS asks (with the Info.plist line, NSCameraUsageDescription) only after the app's own words were shown.
    static func ask() async -> Bool { await AVCaptureDevice.requestAccess(for: .video) }
}
