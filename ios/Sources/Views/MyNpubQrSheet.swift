import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

struct MyNpubQrSheet: View {
    let npub: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                if let img = qrImage(from: npub) {
                    Image(uiImage: img)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 240, maxHeight: 240)
                        .background(Color.white)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .accessibilityIdentifier(TestIds.chatListMyNpubQr)
                } else {
                    Text("Could not generate QR code.")
                        .foregroundStyle(.secondary)
                }

                Text(npub)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(.thinMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .accessibilityIdentifier(TestIds.chatListMyNpubValue)

                Button("Copy") {
                    UIPasteboard.general.string = npub
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier(TestIds.chatListMyNpubCopy)

                Spacer()
            }
            .padding(16)
            .navigationTitle("My npub")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") { dismiss() }
                        .accessibilityIdentifier(TestIds.chatListMyNpubClose)
                }
            }
        }
    }

    private func qrImage(from text: String) -> UIImage? {
        let data = Data(text.utf8)
        let filter = CIFilter.qrCodeGenerator()
        filter.setValue(data, forKey: "inputMessage")
        // Crisp edges, no blur.
        guard var output = filter.outputImage else { return nil }
        output = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        let ctx = CIContext()
        guard let cg = ctx.createCGImage(output, from: output.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}

#if DEBUG
#Preview("My npub") {
    MyNpubQrSheet(npub: "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y")
}
#endif
