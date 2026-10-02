import Foundation
import Security

/// Where connection passwords live. The core never persists them.
public protocol SecretStore: Sendable {
    /// Reads the password (may show a Keychain access prompt).
    func password(for id: ConnectionConfig.ID) -> String?
    /// Whether a password is saved, without reading it (no prompt).
    func hasPassword(for id: ConnectionConfig.ID) -> Bool
    func setPassword(_ password: String, for id: ConnectionConfig.ID) throws
    func deletePassword(for id: ConnectionConfig.ID)
}

public struct KeychainError: Error, LocalizedError {
    public let status: OSStatus
    public var errorDescription: String? {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)"
        return "Keychain: \(message)"
    }
}

/// Generic passwords in the login Keychain, one per connection id.
public struct KeychainSecretStore: SecretStore {
    public let service: String
    /// Services used by earlier builds (old app name / bundle id). Passwords found there move to `service`.
    public let legacyServices: [String]

    public init(
        service: String = "ar.fausto.dbear.connection",
        legacyServices: [String] = ["dev.fausto.dbear.connection", "dev.fausto.dbgui.connection"]
    ) {
        self.service = service
        self.legacyServices = legacyServices
    }

    private func query(_ id: String, service: String? = nil) -> [CFString: Any] {
        [kSecClass: kSecClassGenericPassword, kSecAttrService: service ?? self.service, kSecAttrAccount: id]
    }

    public func password(for id: String) -> String? {
        if let password = read(query(id)) { return password }
        // Saved by an earlier build: move it over, so this only happens once.
        for legacy in legacyServices {
            guard let password = read(query(id, service: legacy)) else { continue }
            if (try? setPassword(password, for: id)) != nil {
                SecItemDelete(query(id, service: legacy) as CFDictionary)
            }
            return password
        }
        return nil
    }

    public func hasPassword(for id: String) -> Bool {
        exists(query(id)) || legacyServices.contains { exists(query(id, service: $0)) }
    }

    private func read(_ query: [CFString: Any]) -> String? {
        var q = query
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private func exists(_ query: [CFString: Any]) -> Bool {
        var q = query
        q[kSecReturnAttributes] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        return SecItemCopyMatching(q as CFDictionary, nil) == errSecSuccess
    }

    public func setPassword(_ password: String, for id: String) throws {
        let data = Data(password.utf8)
        let status = SecItemUpdate(query(id) as CFDictionary, [kSecValueData: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query(id)
            item[kSecValueData] = data
            item[kSecAttrLabel] = "dbear connection"
            let added = SecItemAdd(item as CFDictionary, nil)
            guard added == errSecSuccess else { throw KeychainError(status: added) }
        } else if status != errSecSuccess {
            throw KeychainError(status: status)
        }
    }

    public func deletePassword(for id: String) {
        SecItemDelete(query(id) as CFDictionary)
        for legacy in legacyServices { SecItemDelete(query(id, service: legacy) as CFDictionary) }
    }
}

/// For tests and previews.
public final class InMemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: String]

    public init(_ values: [String: String] = [:]) {
        self.values = values
    }

    public func password(for id: String) -> String? { lock.withLock { values[id] } }
    public func hasPassword(for id: String) -> Bool { lock.withLock { values[id] != nil } }
    public func setPassword(_ password: String, for id: String) throws { lock.withLock { values[id] = password } }
    public func deletePassword(for id: String) { _ = lock.withLock { values.removeValue(forKey: id) } }
}
