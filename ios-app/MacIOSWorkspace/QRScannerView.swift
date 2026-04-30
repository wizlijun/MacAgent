import AVFoundation
import SwiftUI

struct QRScannerView: UIViewRepresentable {
    let onScan: (String) -> Void

    final class PreviewContainer: UIView {
        var previewLayer: AVCaptureVideoPreviewLayer?
        override func layoutSubviews() {
            super.layoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            previewLayer?.frame = bounds
            CATransaction.commit()
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(onScan: onScan) }

    func makeUIView(context: Context) -> PreviewContainer {
        let view = PreviewContainer(frame: .zero)
        let session = AVCaptureSession()
        context.coordinator.session = session

        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return view }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return view }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(context.coordinator, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        view.previewLayer = preview
        context.coordinator.previewLayer = preview

        DispatchQueue.global(qos: .userInitiated).async { session.startRunning() }
        return view
    }

    func updateUIView(_ uiView: PreviewContainer, context: Context) {}

    static func dismantleUIView(_ uiView: PreviewContainer, coordinator: Coordinator) {
        coordinator.session?.stopRunning()
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        let onScan: (String) -> Void
        var session: AVCaptureSession?
        var previewLayer: AVCaptureVideoPreviewLayer?

        init(onScan: @escaping (String) -> Void) { self.onScan = onScan }

        func metadataOutput(_ output: AVCaptureMetadataOutput,
                            didOutput metadataObjects: [AVMetadataObject],
                            from connection: AVCaptureConnection) {
            guard let obj = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  let value = obj.stringValue else { return }
            session?.stopRunning()
            onScan(value)
        }
    }
}
