import SwiftUI

struct PollComposerView: View {
    let onSubmit: (String, [String]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var question = ""
    @State private var options = ["", ""]
    @FocusState private var focusedField: Int?

    private var canSubmit: Bool {
        !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && options.filter({ !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }).count >= 2
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Ask a question...", text: $question)
                        .focused($focusedField, equals: -1)
                }

                Section("Options") {
                    ForEach(options.indices, id: \.self) { index in
                        TextField("Option \(index + 1)", text: $options[index])
                            .focused($focusedField, equals: index)
                    }
                    .onDelete { indexSet in
                        guard options.count > 2 else { return }
                        options.remove(atOffsets: indexSet)
                    }

                    if options.count < 10 {
                        Button {
                            options.append("")
                            focusedField = options.count - 1
                        } label: {
                            Label("Add option", systemImage: "plus")
                        }
                    }
                }

                Section {
                    Button("Create Poll") {
                        onSubmit(
                            question.trimmingCharacters(in: .whitespacesAndNewlines),
                            options
                                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                                .filter { !$0.isEmpty }
                        )
                    }
                    .frame(maxWidth: .infinity)
                    .disabled(!canSubmit)
                }
            }
            .navigationTitle("New Poll")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .onAppear {
                focusedField = -1
            }
        }
    }
}
