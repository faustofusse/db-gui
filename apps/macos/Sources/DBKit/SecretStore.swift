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

    public init(service: String = "dev.fausto.dbgui.connection") {
        self.service = service
    }

    private func query(_ id: String) -> [CFString: Any] {
        [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: id]
    }

    public func password(for id: String) -> String? {
        var q = query(id)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public func hasPassword(for id: String) -> Bool {
        var q = query(id)
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
            item[kSecAttrLabel] = "DBGui connection"
            let added = SecItemAdd(item as CFDictionary, nil)
            guard added == errSecSuccess else { throw KeychainError(status: added) }
        } else if status != errSecSuccess {
            throw KeychainError(status: status)
        }
    }

    public func deletePassword(for id: String) {
        SecItemDelete(query(id) as CFDictionary)
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
