import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

struct MyNpubQrSheet: View {
    let npub: String
    let manager: AppManager
    @Environment(\.dismiss) private var dismiss
    @State private var showNsec = false

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

                Button("Copy npub") {
                    UIPasteboard.general.string = npub
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier(TestIds.chatListMyNpubCopy)

                if let nsec = manager.getNsec() {
                    Divider()
                        .padding(.vertical, 8)

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Private Key (nsec)")
                            .font(.caption)
                            .foregroundStyle(.secondary)

                        HStack {
                            if showNsec {
                                Text(nsec)
                                    .font(.system(.footnote, design: .monospaced))
                                    .textSelection(.enabled)
                            } else {
                                Text(String(repeating: "•", count: 24))
                                    .font(.system(.footnote, design: .monospaced))
                            }
                            Spacer()
                            Button {
                                showNsec.toggle()
                            } label: {
                                Image(systemName: showNsec ? "eye.slash" : "eye")
                            }
                            .accessibilityIdentifier(TestIds.myNpubNsecToggle)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(.thinMaterial)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .accessibilityIdentifier(TestIds.myNpubNsecValue)

                        Button("Copy nsec") {
                            UIPasteboard.general.string = nsec
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier(TestIds.myNpubNsecCopy)
                    }
                }

                Spacer()
            }
            .padding(16)
            .navigationTitle("My Profile")
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

