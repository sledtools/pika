import SwiftUI

struct FileAttachmentRow: View {
    let filename: String
    let mimeType: String
    let localPath: String?
    let isMine: Bool
    var onDownload: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "doc")
                .font(.title3)
                .foregroundStyle(isMine ? .white.opacity(0.8) : .secondary)

            VStack(alignment: .leading, spacing: 2) {
                Text(filename)
                    .font(.subheadline)
                    .foregroundStyle(isMine ? .white : .primary)
                    .lineLimit(1)
                Text(mimeType)
                    .font(.caption2)
                    .foregroundStyle(isMine ? .white.opacity(0.6) : .secondary)
            }

            Spacer(minLength: 4)

            if let localPath {
                ShareLink(item: URL(fileURLWithPath: localPath)) {
                    Image(systemName: "square.and.arrow.up")
                        .font(.title3)
                        .foregroundStyle(isMine ? .white : .blue)
                }
                .buttonStyle(.plain)
            } else {
                Button {
                    onDownload?()
                } label: {
                    Image(systemName: "arrow.down.circle")
                        .font(.title3)
                        .foregroundStyle(isMine ? .white : .blue)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }
}

#if DEBUG
#Preview("File Row — Mine, Downloaded") {
    FileAttachmentRow(
        filename: "sunset pirate.txt",
        mimeType: "text/plain",
        localPath: "/tmp/fake",
        isMine: true
    )
    .background(Color.blue)
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .padding()
}

#Preview("File Row — Mine, Not Downloaded") {
    FileAttachmentRow(
        filename: "quarterly-report.pdf",
        mimeType: "application/pdf",
        localPath: nil,
        isMine: true
    )
    .background(Color.blue)
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .padding()
}

#Preview("File Row — Incoming, Downloaded") {
    FileAttachmentRow(
        filename: "meeting-notes.pdf",
        mimeType: "application/pdf",
        localPath: "/tmp/fake",
        isMine: false
    )
    .background(Color.gray.opacity(0.2))
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .padding()
}

#Preview("File Row — Incoming, Not Downloaded") {
    FileAttachmentRow(
        filename: "archive.zip",
        mimeType: "application/zip",
        localPath: nil,
        isMine: false
    )
    .background(Color.gray.opacity(0.2))
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .padding()
}

#Preview("File Row — Long Filename") {
    FileAttachmentRow(
        filename: "a-very-long-filename-that-should-be-truncated-with-ellipsis.docx",
        mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        localPath: "/tmp/fake",
        isMine: true
    )
    .frame(maxWidth: 280)
    .background(Color.blue)
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .padding()
}
#endif
