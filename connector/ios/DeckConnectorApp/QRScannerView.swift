import AVFoundation
import SwiftUI
import UIKit

struct QRScannerView: UIViewControllerRepresentable {
    let completion: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(completion: completion) }
    func makeUIViewController(context: Context) -> ScannerController { let controller = ScannerController(); controller.coordinator = context.coordinator; return controller }
    func updateUIViewController(_ uiViewController: ScannerController, context: Context) { }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        let completion: (String) -> Void
        private var completed = false
        init(completion: @escaping (String) -> Void) { self.completion = completion }
        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
            guard !completed, let code = metadataObjects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first,
                  code.hasPrefix("deck-connector://pair?") else { return }
            completed = true
            completion(code)
        }
    }
}

final class ScannerController: UIViewController {
    var coordinator: QRScannerView.Coordinator?
    private let session = AVCaptureSession()
    private let captureQueue = DispatchQueue(label: "io.c9r.deck.connector.camera")
    private var configured = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        requestCamera()
    }

    private func requestCamera() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configureAndStart()
        case .notDetermined:
            Task {
                let granted = await AVCaptureDevice.requestAccess(for: .video)
                if granted { configureAndStart() } else { showUnavailable() }
            }
        default:
            showUnavailable()
        }
    }

    private func configureAndStart() {
        guard !configured else { return }
        configured = true
        guard let camera = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: camera), session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(coordinator, queue: .main)
        output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.addSublayer(layer)
        captureQueue.async { [session] in session.startRunning() }
    }

    private func showUnavailable() {
        let label = UILabel(frame: view.bounds.insetBy(dx: 24, dy: 24))
        label.text = "Camera access is unavailable. Cancel and paste the Deck pairing code instead."
        label.textColor = .white
        label.textAlignment = .center
        label.numberOfLines = 0
        view.addSubview(label)
    }

    override func viewDidLayoutSubviews() { super.viewDidLayoutSubviews(); (view.layer.sublayers?.first as? AVCaptureVideoPreviewLayer)?.frame = view.bounds }
    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        captureQueue.async { [session] in if session.isRunning { session.stopRunning() } }
    }
}
