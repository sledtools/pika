import SwiftUI

struct AvatarView: View {
    let name: String?
    let npub: String
    let pictureUrl: String?
    var size: CGFloat = 44

    var body: some View {
        if let url = pictureUrl.flatMap({ URL(string: $0) }) {
            AsyncImage(url: url) { image in
                image.resizable().scaledToFill()
            } placeholder: {
                initialsCircle
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
        } else {
            initialsCircle
        }
    }

    private var initialsCircle: some View {
        Circle()
            .fill(Color.blue.opacity(0.12))
            .frame(width: size, height: size)
            .overlay {
                Text(initials)
                    .font(.system(size: size * 0.4, weight: .medium))
                    .foregroundStyle(.blue)
            }
    }

    private var initials: String {
        let source = name ?? npub
        return String(source.prefix(1)).uppercased()
    }
}

#if DEBUG
#Preview("Avatar - Initials") {
    AvatarView(name: "Pika", npub: "npub1example", pictureUrl: nil, size: 56)
        .padding()
}
#endif
