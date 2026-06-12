import JCodeKit
import SwiftUI

/// Main conversation screen.
struct ChatView: View {
    @Environment(AppModel.self) private var model
    @State private var showSettings = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header

            if let banner = model.session.errorBanner {
                ErrorBanner(message: banner) {
                    model.dismissError()
                }
                .padding(.bottom, 8)
            }

            TranscriptView(
                entries: model.session.transcript,
                isReasoning: model.session.isReasoning
            )

            Composer(
                draft: $model.draft,
                isProcessing: model.session.isProcessing,
                isConnected: model.isConnected,
                onSend: { model.sendDraft() },
                onInterrupt: { model.interrupt() }
            )
        }
        .sheet(isPresented: $showSettings) {
            SettingsView()
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.session.sessionTitle ?? model.activeServer?.serverName ?? "jcode")
                    .font(Theme.mono(16, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                if let modelName = model.session.modelName {
                    Text(modelName)
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                }
            }
            Spacer()
            StatusPill(phase: model.session.phase)
            Button {
                showSettings = true
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(.title3)
                    .foregroundStyle(Theme.textSecondary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}

/// Scrolling transcript with auto-follow.
struct TranscriptView: View {
    let entries: [TranscriptEntry]
    let isReasoning: Bool

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    ForEach(entries) { entry in
                        EntryView(entry: entry)
                            .id(entry.id)
                    }
                    if isReasoning {
                        HStack(spacing: 8) {
                            ProgressView()
                                .controlSize(.small)
                                .tint(Theme.textTertiary)
                            Text("thinking")
                                .font(Theme.mono(12))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        .padding(.leading, 4)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: entries.last?.text) {
                withAnimation(.easeOut(duration: 0.15)) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .onChange(of: entries.count) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
    }
}

/// One transcript entry: user bubble, assistant markdown, or system note.
struct EntryView: View {
    let entry: TranscriptEntry

    var body: some View {
        switch entry.role {
        case .user:
            HStack {
                Spacer(minLength: 48)
                Text(entry.text)
                    .font(.body)
                    .foregroundStyle(Theme.textPrimary)
                    .padding(12)
                    .background(Theme.mintTint)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
            }
        case .assistant:
            VStack(alignment: .leading, spacing: 8) {
                if !entry.reasoning.isEmpty {
                    Text(entry.reasoning)
                        .font(Theme.mono(12))
                        .italic()
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(4)
                }
                ForEach(entry.toolCalls) { call in
                    ToolCallCard(call: call)
                }
                if !entry.text.isEmpty {
                    MarkdownText(entry.text)
                }
            }
        case .system:
            Text(entry.text)
                .font(.footnote)
                .foregroundStyle(Theme.textTertiary)
                .frame(maxWidth: .infinity, alignment: .center)
        }
    }
}

/// Collapsible tool call card with live status.
struct ToolCallCard: View {
    let call: TranscriptEntry.ToolCall
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    expanded.toggle()
                }
            } label: {
                HStack(spacing: 8) {
                    statusIcon
                    Text(call.name)
                        .font(Theme.mono(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            if expanded {
                if !call.input.isEmpty {
                    codeBlock(call.input)
                }
                if !call.output.isEmpty {
                    codeBlock(String(call.output.prefix(2000)))
                }
                if case let .failed(message) = call.status {
                    Text(message)
                        .font(Theme.mono(12))
                        .foregroundStyle(Theme.error)
                }
            }
        }
        .padding(10)
        .background(Theme.surfaceElevated)
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch call.status {
        case .streamingInput, .running:
            ProgressView()
                .controlSize(.mini)
                .tint(Theme.mint)
        case .succeeded:
            Image(systemName: "checkmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Theme.mint)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Theme.error)
        }
    }

    private func codeBlock(_ text: String) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(text)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textSecondary)
                .padding(8)
        }
        .background(Theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

/// Message composer with send/interrupt.
struct Composer: View {
    @Binding var draft: String
    let isProcessing: Bool
    let isConnected: Bool
    let onSend: () -> Void
    let onInterrupt: () -> Void

    var body: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField(
                isProcessing ? "Queue a message..." : "Message",
                text: $draft,
                axis: .vertical
            )
            .lineLimit(1...6)
            .font(.body)
            .foregroundStyle(Theme.textPrimary)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(Theme.surface)
            .clipShape(RoundedRectangle(cornerRadius: 20))
            .overlay(
                RoundedRectangle(cornerRadius: 20)
                    .stroke(Theme.border, lineWidth: 1)
            )

            if isProcessing {
                Button(action: onInterrupt) {
                    Image(systemName: "stop.fill")
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Theme.error)
                        .frame(width: 40, height: 40)
                        .background(Theme.surface)
                        .clipShape(Circle())
                }
            }

            Button(action: onSend) {
                Image(systemName: "arrow.up")
                    .font(.body.weight(.bold))
                    .foregroundStyle(.black)
                    .frame(width: 40, height: 40)
                    .background(canSend ? Theme.mint : Theme.surfaceElevated)
                    .clipShape(Circle())
            }
            .disabled(!canSend)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Theme.background)
    }

    private var canSend: Bool {
        isConnected && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}
