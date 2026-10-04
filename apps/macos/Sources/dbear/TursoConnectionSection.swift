import DBKit
import SwiftUI

/// The connection editor's fields for Turso / libSQL: a host instead of a database, an auth token
/// instead of a user and password (stored in the Keychain like one), and how to reach the server.
struct TursoConnectionSection<KindPicker: View>: View {
    @Binding var draft: ConnectionConfig
    /// The editor's password field: for Turso it holds the auth token.
    @Binding var token: String
    let tokenPrompt: String
    /// Called when the token is typed or pasted, so the editor saves it.
    let tokenEdited: () -> Void
    @ViewBuilder let kindPicker: KindPicker

    var body: some View {
        Section("Server") {
            kindPicker
            TextField("Host", text: host, prompt: Text(verbatim: "mydb-org.turso.io"))
            TextField("Port", value: $draft.port, format: .number.grouping(.never), prompt: Text("Optional"))
            Picker("Connection", selection: $draft.sslMode) {
                Text("HTTPS").tag(SslMode.verifyFull)
                Text("HTTPS, don't verify certificate").tag(SslMode.require)
                Text("HTTP (local server)").tag(SslMode.disable)
            }
        }
        Section {
            SecureField("Auth Token", text: $token, prompt: Text(tokenPrompt))
                .onChange(of: token) { tokenEdited() }
        } header: {
            Text("Authentication")
        } footer: {
            Text("Create one with `turso db tokens create <database>`. Tokens are stored in your login Keychain, never in the connections file.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    /// Pasting a whole `libsql://…?authToken=…` URL here fills in every field.
    private var host: Binding<String> {
        Binding(
            get: { draft.host },
            set: { value in
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                guard trimmed.contains("://"), let parsed = try? ConnectionConfig.parse(url: trimmed), parsed.kind == .libsql
                else {
                    draft.host = value
                    return
                }
                draft.host = parsed.host
                draft.port = parsed.port
                draft.sslMode = parsed.sslMode
                if let parsedToken = parsed.password { token = parsedToken }
            }
        )
    }
}
