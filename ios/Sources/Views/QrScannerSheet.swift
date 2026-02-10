import AVFoundation
import SwiftUI

struct QrScannerSheet: View {
    let onScanned: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var authStatus = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var errorMessage: String?
    @State private var scannerNonce = UUID()

    var body: some View {
        NavigationStack {
            VStack(spacing: 12) {
                if let msg = errorMessage {
                    Text(msg)
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }

                content
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .overlay(
                        RoundedRectangle(cornerRadius: 12)
                            .stroke(.secondary.opacity(0.3), lineWidth: 1)
                    )

                Spacer()
            }
            .padding(16)
            .navigationTitle("Scan QR")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") { dismiss() }
                }
            }
            .onAppear { ensureCameraPermission() }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch authStatus {
        case .authorized:
            QrScannerView { raw in
                let normalized = PeerKeyValidator.normalize(raw)
                if PeerKeyValidator.isValidPeer(normalized) {
                    onScanned(normalized)
                    dismiss()
                } else {
                    errorMessage = "Scanned QR is not a valid npub."
                    // Restart capture (avoid being stuck after an invalid scan).
                    scannerNonce = UUID()
                }
            }
            .id(scannerNonce)
            .frame(maxWidth: .infinity)
            .aspectRatio(1, contentMode: .fit)

        case .notDetermined:
            ProgressView("Requesting camera permission…")
                .frame(maxWidth: .infinity, minHeight: 240)

        case .denied, .restricted:
            VStack(spacing: 8) {
                Text("Camera permission is required to scan QR codes.")
                    .foregroundStyle(.secondary)
                Text("Use Paste on the New Chat screen instead.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, minHeight: 240)

        @unknown default:
            Text("Camera unavailable.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, minHeight: 240)
        }
    }

    private func ensureCameraPermission() {
        let status = AVCaptureDevice.authorizationStatus(for: .video)
        authStatus = status
        guard status == .notDetermined else { return }
        AVCaptureDevice.requestAccess(for: .video) { granted in
            DispatchQueue.main.async {
                authStatus = granted ? .authorized : .denied
            }
        }
    }
}

private struct QrScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> QrScannerViewController {
        let vc = QrScannerViewController()
        vc.onCode = onCode
        return vc
    }

    func updateUIViewController(_ uiViewController: QrScannerViewController, context: Context) {}
}

private final class QrScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var didEmit = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let device = AVCaptureDevice.default(for: .video) else { return }
        guard let input = try? AVCaptureDeviceInput(device: device) else { return }
        guard session.canAddInput(input) else { return }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: DispatchQueue.main)
        output.metadataObjectTypes = [.qr]

        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        previewLayer = layer
        view.layer.addSublayer(layer)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        didEmit = false
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.startRunning()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.stopRunning()
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !didEmit else { return }
        guard let obj = metadataObjects.first as? AVMetadataMachineReadableCodeObject else { return }
        guard obj.type == .qr, let value = obj.stringValue, !value.isEmpty else { return }
        didEmit = true
        onCode?(value)
    }
}

