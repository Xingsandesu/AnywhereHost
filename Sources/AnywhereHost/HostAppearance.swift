import SwiftUI

struct HostSheet<Content: View>: View {
    let title: LocalizedStringKey
    @ViewBuilder let content: Content
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            content
                .navigationTitle(title)
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("完成") { dismiss() }.keyboardShortcut(.defaultAction)
                    }
                }
        }
    }
}

struct HostPrimaryAction: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            content.buttonStyle(.glassProminent)
        } else {
            content.buttonStyle(.borderedProminent)
        }
    }
}

struct HostStatusLabel: View {
    let connected: Bool
    var body: some View {
        Label(connected ? "在线" : "离线", systemImage: connected ? "checkmark.circle.fill" : "circle.dotted")
            .foregroundStyle(connected ? Color.green : Color.secondary)
            .font(.callout)
    }
}
