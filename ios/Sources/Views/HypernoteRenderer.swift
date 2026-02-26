import SwiftUI

// MARK: - AST Types

private struct HypernoteAstNode: Decodable {
    let type: String
    var value: String?
    var children: [HypernoteAstNode]?
    var level: Int?
    var url: String?
    var lang: String?
    var name: String?
    var attributes: [HypernoteAstAttribute]?
}

private struct HypernoteAstAttribute: Decodable {
    let name: String
    let type: String
    var value: String?
}

// MARK: - Renderer

struct HypernoteRenderer: View {
    let astJson: String
    let messageId: String
    let defaultState: String?
    let myResponse: String?
    let responseTallies: [HypernoteResponseTally]
    let responders: [HypernoteResponder]
    let onAction: (String, String, [String: String]) -> Void

    @State private var interactionState: [String: String] = [:]
    @State private var localSubmittedAction: String?
    @State private var hasInitialized = false
    private var selectedAction: String? { myResponse ?? localSubmittedAction }
    private var isSubmitted: Bool { selectedAction != nil }

    var body: some View {
        Group {
            if let root = parseAst() {
                VStack(alignment: .leading, spacing: 8) {
                    if let children = root.children {
                        ForEach(Array(children.enumerated()), id: \.offset) { _, node in
                            renderNode(node)
                        }
                    }
                    if !responders.isEmpty {
                        HStack(spacing: -6) {
                            ForEach(responders.prefix(5), id: \.npub) { responder in
                                AvatarView(
                                    name: responder.name,
                                    npub: responder.npub,
                                    pictureUrl: responder.pictureUrl,
                                    size: 20
                                )
                                .overlay(Circle().stroke(.background, lineWidth: 1.5))
                            }
                            if responders.count > 5 {
                                Text("+\(responders.count - 5)")
                                    .font(.caption2.weight(.medium))
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .padding(.top, 4)
                    }
                }
            } else {
                Text("Failed to parse hypernote")
                    .foregroundStyle(.secondary)
                    .italic()
            }
        }
        .opacity(isSubmitted ? 0.8 : 1.0)
        .padding(12)
        .onAppear {
            guard !hasInitialized else { return }
            hasInitialized = true
            if let json = defaultState,
               let data = json.data(using: .utf8),
               let dict = try? JSONSerialization.jsonObject(with: data) as? [String: String] {
                for (key, value) in dict {
                    interactionState[key] = value
                }
            }
        }
    }

    private func parseAst() -> HypernoteAstNode? {
        guard let data = astJson.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(HypernoteAstNode.self, from: data)
    }

    // MARK: - Node Rendering
    //
    // AnyView is used here intentionally to break the recursive opaque-type
    // chain that causes the Swift type checker to hang during compilation.

    private func renderNode(_ node: HypernoteAstNode) -> AnyView {
        switch node.type {
        case "heading":
            AnyView(renderHeading(node))
        case "paragraph":
            AnyView(renderParagraph(node))
        case "strong":
            AnyView(inlineText(from: node.children).bold())
        case "emphasis":
            AnyView(inlineText(from: node.children).italic())
        case "code_inline":
            AnyView(
                Text(node.value ?? "")
                    .font(.system(.body, design: .monospaced))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 2)
                    .background(Color.gray.opacity(0.15))
                    .clipShape(.rect(cornerRadius: 4))
            )
        case "code_block":
            AnyView(renderCodeBlock(node))
        case "link":
            AnyView(renderLink(node))
        case "image":
            AnyView(renderImage(node))
        case "list_unordered":
            AnyView(renderList(node, ordered: false))
        case "list_ordered":
            AnyView(renderList(node, ordered: true))
        case "list_item":
            AnyView(renderChildren(node))
        case "blockquote":
            AnyView(renderBlockquote(node))
        case "hr":
            AnyView(Divider())
        case "hard_break":
            AnyView(Spacer().frame(height: 4))
        case "text":
            AnyView(Text(node.value ?? ""))
        case "mdx_jsx_element", "mdx_jsx_self_closing":
            renderJsxComponent(node)
        default:
            AnyView(renderChildren(node))
        }
    }

    @ViewBuilder
    private func renderChildren(_ node: HypernoteAstNode) -> some View {
        if let children = node.children {
            ForEach(Array(children.enumerated()), id: \.offset) { _, child in
                renderNode(child)
            }
        }
    }

    // MARK: - Markdown Nodes

    private func renderHeading(_ node: HypernoteAstNode) -> some View {
        let font: Font = switch node.level ?? 1 {
        case 1: .title
        case 2: .title2
        case 3: .title3
        default: .headline
        }
        return inlineText(from: node.children)
            .font(font)
            .bold()
    }

    @ViewBuilder
    private func renderParagraph(_ node: HypernoteAstNode) -> some View {
        if hasOnlyInlineChildren(node) {
            inlineText(from: node.children)
        } else if let children = node.children {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(children.enumerated()), id: \.offset) { _, child in
                    renderNode(child)
                }
            }
        }
    }

    private func renderCodeBlock(_ node: HypernoteAstNode) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if let lang = node.lang {
                Text(lang)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.top, 6)
            }
            Text(node.value ?? "")
                .font(.system(.caption, design: .monospaced))
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color.gray.opacity(0.1))
        .clipShape(.rect(cornerRadius: 8))
    }

    @ViewBuilder
    private func renderLink(_ node: HypernoteAstNode) -> some View {
        if let urlStr = node.url, let url = URL(string: urlStr) {
            let label = extractText(from: node.children).isEmpty ? urlStr : extractText(from: node.children)
            Link(label, destination: url)
        } else {
            inlineText(from: node.children)
        }
    }

    @ViewBuilder
    private func renderImage(_ node: HypernoteAstNode) -> some View {
        if let urlStr = node.url, let url = URL(string: urlStr) {
            AsyncImage(url: url) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .clipShape(.rect(cornerRadius: 8))
                case .failure:
                    Label("Image failed to load", systemImage: "photo")
                        .foregroundStyle(.secondary)
                default:
                    ProgressView()
                        .frame(height: 100)
                }
            }
        }
    }

    private func renderList(_ node: HypernoteAstNode, ordered: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let items = node.children {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .top, spacing: 6) {
                        Text(ordered ? "\(index + 1)." : "\u{2022}")
                            .foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 2) {
                            if let children = item.children {
                                ForEach(Array(children.enumerated()), id: \.offset) { _, child in
                                    renderNode(child)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func renderBlockquote(_ node: HypernoteAstNode) -> some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.gray.opacity(0.4))
                .frame(width: 3)
                .clipShape(.rect(cornerRadius: 1.5))
            VStack(alignment: .leading, spacing: 4) {
                if let children = node.children {
                    ForEach(Array(children.enumerated()), id: \.offset) { _, child in
                        renderNode(child)
                    }
                }
            }
            .padding(.leading, 10)
        }
        .padding(.vertical, 4)
    }

    // MARK: - JSX Components

    private func renderJsxComponent(_ node: HypernoteAstNode) -> AnyView {
        let name = node.name ?? ""
        let attrs = attributeDict(node.attributes)

        switch name {
        case "Card":
            return AnyView(
                VStack(alignment: .leading, spacing: 8) {
                    renderChildren(node)
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.gray.opacity(0.08))
                .clipShape(.rect(cornerRadius: 12))
            )

        case "VStack":
            let spacing = CGFloat(Int(attrs["spacing"] ?? attrs["gap"] ?? "8") ?? 8)
            return AnyView(
                VStack(alignment: .leading, spacing: spacing) {
                    renderChildren(node)
                }
            )

        case "HStack":
            let spacing = CGFloat(Int(attrs["spacing"] ?? attrs["gap"] ?? "8") ?? 8)
            return AnyView(
                HStack(spacing: spacing) {
                    renderChildren(node)
                }
            )

        case "Heading":
            return AnyView(
                inlineText(from: node.children)
                    .font(.headline)
            )

        case "Body":
            return AnyView(
                inlineText(from: node.children)
                    .font(.body)
            )

        case "Caption":
            return AnyView(
                inlineText(from: node.children)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            )

        case "TextInput":
            let fieldName = attrs["name"] ?? "field"
            let placeholder = attrs["placeholder"] ?? ""
            return AnyView(
                TextField(placeholder, text: Binding(
                    get: { interactionState[fieldName] ?? "" },
                    set: { interactionState[fieldName] = $0 }
                ))
                .textFieldStyle(.roundedBorder)
                .disabled(isSubmitted)
            )

        case "SubmitButton":
            let actionName = attrs["action"] ?? "submit"
            let variant = attrs["variant"] ?? "primary"
            let isSelected = selectedAction == actionName
            let isUnselected = isSubmitted && !isSelected
            let useProminent: Bool = switch variant {
            case "secondary": isSelected
            default: !isSubmitted || isSelected
            }
            let tally = responseTallies.first(where: { $0.action == actionName })
            let buttonLabel = HStack(spacing: 6) {
                if isSelected {
                    Image(systemName: "checkmark")
                        .font(.caption.weight(.bold))
                }
                inlineText(from: node.children)
                if let tally {
                    Text("\(tally.count)")
                        .font(.caption.weight(.semibold))
                }
            }
            .frame(maxWidth: .infinity)

            let buttonAction = {
                let formData = interactionState
                localSubmittedAction = actionName
                onAction(actionName, messageId, formData)
            }

            if useProminent {
                return AnyView(
                    Button(action: buttonAction) { buttonLabel }
                        .buttonStyle(.borderedProminent)
                        .tint(variant == "danger" ? .red : nil)
                        .disabled(isSubmitted)
                        .opacity(isUnselected ? 0.5 : 1.0)
                )
            } else {
                return AnyView(
                    Button(action: buttonAction) { buttonLabel }
                        .buttonStyle(.bordered)
                        .disabled(isSubmitted)
                        .opacity(isUnselected ? 0.5 : 1.0)
                )
            }

        case "ChecklistItem":
            let fieldName = attrs["name"] ?? "item"
            let defaultChecked = attrs["checked"] != nil
            let isChecked = interactionState[fieldName] == "true"
            return AnyView(
                Button {
                    interactionState[fieldName] = isChecked ? "false" : "true"
                } label: {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: isChecked ? "checkmark.square.fill" : "square")
                            .foregroundStyle(isChecked ? .blue : .secondary)
                            .font(.body)
                        inlineText(from: node.children)
                            .strikethrough(isChecked)
                            .foregroundStyle(isChecked ? .secondary : .primary)
                            .multilineTextAlignment(.leading)
                    }
                }
                .buttonStyle(.plain)
                .disabled(isSubmitted)
                .onAppear {
                    if interactionState[fieldName] == nil {
                        interactionState[fieldName] = defaultChecked ? "true" : "false"
                    }
                }
            )

        default:
            return AnyView(
                VStack(alignment: .leading, spacing: 4) {
                    renderChildren(node)
                }
                .padding(8)
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .strokeBorder(style: StrokeStyle(lineWidth: 1, dash: [4]))
                        .foregroundStyle(.tertiary)
                )
            )
        }
    }

    // MARK: - Helpers

    private func attributeDict(_ attrs: [HypernoteAstAttribute]?) -> [String: String] {
        guard let attrs else { return [:] }
        var dict: [String: String] = [:]
        for attr in attrs {
            dict[attr.name] = attr.value ?? ""
        }
        return dict
    }

    private func hasOnlyInlineChildren(_ node: HypernoteAstNode) -> Bool {
        guard let children = node.children else { return true }
        let inlineTypes: Set<String> = ["text", "strong", "emphasis", "code_inline", "link", "hard_break"]
        return children.allSatisfy { inlineTypes.contains($0.type) }
    }

    private func inlineText(from children: [HypernoteAstNode]?) -> Text {
        guard let children else { return Text("") }
        return children.reduce(Text("")) { result, child in
            result + inlineTextNode(child)
        }
    }

    private func inlineTextNode(_ node: HypernoteAstNode) -> Text {
        switch node.type {
        case "text":
            return Text(node.value ?? "")
        case "strong":
            return inlineText(from: node.children).bold()
        case "emphasis":
            return inlineText(from: node.children).italic()
        case "code_inline":
            return Text(node.value ?? "").font(.system(.body, design: .monospaced))
        case "link":
            let label = extractText(from: node.children)
            return Text(label).underline().foregroundColor(.blue)
        case "hard_break":
            return Text("\n")
        default:
            return Text(node.value ?? "")
        }
    }

    private func extractText(from children: [HypernoteAstNode]?) -> String {
        guard let children else { return "" }
        return children.map { node in
            if let value = node.value { return value }
            return extractText(from: node.children)
        }.joined()
    }
}
