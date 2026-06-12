import JCodeKit
import SwiftUI

/// Settings sheet: sessions, model picker, servers, session info.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var renameDraft = ""
    @State private var showRename = false
    @State private var showPairNew = false

    var body: some View {
        NavigationStack {
            List {
                sessionsSection
                modelSection
                serversSection
                infoSection
            }
            .scrollContentBackground(.hidden)
            .background(Theme.background)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .preferredColorScheme(.dark)
        .alert("Rename session", isPresented: $showRename) {
            TextField("Title", text: $renameDraft)
            Button("Rename") {
                model.renameSession(renameDraft)
            }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(isPresented: $showPairNew) {
            NavigationStack {
                PairingView()
                    .background(Theme.background)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { showPairNew = false }
                        }
                    }
            }
            .preferredColorScheme(.dark)
        }
        .onChange(of: model.activeServer?.id) {
            showPairNew = false
        }
    }

    private var sessionsSection: some View {
        Section("Sessions") {
            ForEach(model.session.allSessions, id: \.self) { sessionID in
                Button {
                    model.switchSession(sessionID)
                    dismiss()
                } label: {
                    HStack {
                        Text(shortSessionID(sessionID))
                            .font(Theme.mono(13))
                            .foregroundStyle(Theme.textPrimary)
                        Spacer()
                        if sessionID == model.session.sessionID {
                            Image(systemName: "checkmark")
                                .font(.caption)
                                .foregroundStyle(Theme.mint)
                        }
                    }
                }
                .listRowBackground(Theme.surface)
            }
            Button {
                renameDraft = model.session.sessionTitle ?? ""
                showRename = true
            } label: {
                Label("Rename current session", systemImage: "pencil")
                    .foregroundStyle(Theme.textPrimary)
            }
            .listRowBackground(Theme.surface)
        }
    }

    private var modelSection: some View {
        Section("Model") {
            ForEach(model.session.availableModels, id: \.self) { name in
                Button {
                    model.setModel(name)
                } label: {
                    HStack {
                        Text(name)
                            .font(Theme.mono(13))
                            .foregroundStyle(Theme.textPrimary)
                            .lineLimit(1)
                        Spacer()
                        if name == model.session.modelName {
                            Image(systemName: "checkmark")
                                .font(.caption)
                                .foregroundStyle(Theme.mint)
                        }
                    }
                }
                .listRowBackground(Theme.surface)
            }
        }
    }

    private var serversSection: some View {
        Section("Servers") {
            ForEach(model.servers) { server in
                Button {
                    model.connect(to: server)
                    dismiss()
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(server.serverName)
                                .foregroundStyle(Theme.textPrimary)
                            Text("\(server.host):\(String(server.port))")
                                .font(Theme.mono(11))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        Spacer()
                        if server.id == model.activeServer?.id {
                            Circle()
                                .fill(Theme.mint)
                                .frame(width: 8, height: 8)
                        }
                    }
                }
                .listRowBackground(Theme.surface)
                .swipeActions {
                    Button(role: .destructive) {
                        model.removeServer(server)
                    } label: {
                        Label("Remove", systemImage: "trash")
                    }
                }
            }
            Button {
                showPairNew = true
            } label: {
                Label("Pair new server", systemImage: "plus")
                    .foregroundStyle(Theme.mint)
            }
            .listRowBackground(Theme.surface)
        }
    }

    private var infoSection: some View {
        Section("Info") {
            row("Server version", model.session.serverVersion ?? "unknown")
            row("Provider", model.session.providerName ?? "unknown")
            row(
                "Tokens",
                "\(model.session.tokenInput) in / \(model.session.tokenOutput) out"
            )
            if let detail = model.session.statusDetail {
                row("Status", detail)
            }
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
                .foregroundStyle(Theme.textSecondary)
            Spacer()
            Text(value)
                .font(Theme.mono(12))
                .foregroundStyle(Theme.textTertiary)
                .lineLimit(1)
        }
        .listRowBackground(Theme.surface)
    }

    private func shortSessionID(_ id: String) -> String {
        if id.count > 24 {
            return String(id.prefix(24)) + "…"
        }
        return id
    }
}
