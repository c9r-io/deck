import Foundation

public enum ConnectorLimits {
    public static let requestBytes = 256 * 1024
    public static let responseBytes = 4 * 1024 * 1024
    public static let pairingDescriptorBytes = 8 * 1024
    public static let serializedBufferBytes = 2 * 1024 * 1024
    public static let journalBytes = 2 * 1024 * 1024
    public static let terminalTextUTF8Bytes = 64 * 1024
    public static let bufferEntries = 256
    public static let bufferCopies = 256
    public static let bufferEntryUTF8Bytes = 32 * 1024
    public static let bufferTotalUTF8Bytes = 1024 * 1024
    public static let commandJournalEntries = 1_000
    public static let commandResultBytes = 1024
    public static let terminalResultReserveBytes = 1_536
    public static let commandTextUTF8Bytes = 32 * 1024
    public static let draftUTF8Bytes = commandTextUTF8Bytes
}

public enum ConnectorError: Error, Equatable, LocalizedError, Sendable {
    case invalidPairingDescriptor
    case expiredPairingDescriptor
    case invalidOrigin
    case responseTooLarge
    case requestTooLarge
    /// Command text the host would refuse (empty, or a control character
    /// other than newline and tab); caught before anything is sent.
    case invalidCommandText
    case invalidResponse
    case certificateMismatch
    case untrustedServer
    case redirected
    case revoked
    case conflict(String)
    case commandNotFound
    /// 410: the host no longer holds this operation's outcome. It may or may
    /// not have run; it is never sent again.
    case commandExpired
    case capacityExceeded
    case upgradeRequired
    case unsupportedTarget
    case transport(String)

    public var errorDescription: String? {
        switch self {
        case .invalidPairingDescriptor: "The pairing code is invalid."
        case .expiredPairingDescriptor: "The pairing code expired. Create a new one on the desktop."
        case .invalidOrigin: "The Deck host address is invalid."
        case .responseTooLarge: "The host response exceeded the client limit."
        case .requestTooLarge: "The request exceeded the client limit."
        case .invalidCommandText: "The text is empty or contains a control character (such as a carriage return). Remove it and send again."
        case .invalidResponse: "The host returned an invalid response."
        case .certificateMismatch: "The Deck host certificate changed. Pair again from the desktop."
        case .untrustedServer: "The Deck host certificate failed hostname or validity checks."
        case .redirected: "The Deck host attempted a redirect."
        case .revoked: "This phone was revoked by the Deck host."
        case let .conflict(code): "The host rejected stale state: \(code). Refresh before retrying."
        case .commandNotFound: "The host has no record of this operation. You may retry its original immutable ID and body."
        case .commandExpired: "The host no longer keeps this operation's outcome. It may or may not have run, and it will not be sent again."
        case .capacityExceeded: "The local recovery archive has no safe space for another operation while unresolved operations and drafts are preserved."
        case .upgradeRequired: "The Deck host requires a newer version of this app. Update it before sending."
        case .unsupportedTarget: "This action is available only for a card whose saved desktop command is exactly Codex or Claude."
        case let .transport(message): message
        }
    }
}

public struct PairingPayload: Codable, Equatable, Sendable {
    public let version: Int
    public let hostId: String
    public let hostName: String
    public let origin: String
    public let fingerprint: String
    public let code: String
    public let expiresAt: Int64

    public init(version: Int, hostId: String, hostName: String, origin: String, fingerprint: String, code: String, expiresAt: Int64) {
        self.version = version
        self.hostId = hostId
        self.hostName = hostName
        self.origin = origin
        self.fingerprint = fingerprint
        self.code = code
        self.expiresAt = expiresAt
    }
}

public struct PairRequest: Codable, Sendable {
    public let code: String
    public let deviceName: String
    public init(code: String, deviceName: String) { self.code = code; self.deviceName = deviceName }
}

public struct PairResponse: Codable, Sendable {
    public let version: Int
    public let hostId: String
    public let deviceId: String
    public let token: String
}

public struct DeviceCredential: Codable, Equatable, Sendable {
    public let origin: String
    public let fingerprint: String
    public let hostId: String
    public let deviceId: String
    public let token: String

    public init(origin: String, fingerprint: String, hostId: String, deviceId: String, token: String) {
        self.origin = origin
        self.fingerprint = fingerprint
        self.hostId = hostId
        self.deviceId = deviceId
        self.token = token
    }

    public var journalBinding: JournalBinding { JournalBinding(hostID: hostId, deviceID: deviceId) }
}

public struct Snapshot: Codable, Equatable, Sendable {
    public let version: Int
    public let hostId: String
    public let revision: WireRevision
    public let capturedAt: WireInstant
    public let projects: [ProjectSummary]
    public let cards: [CardSummary]
    public let queue: [QueueItem]
}

public struct ProjectSummary: Codable, Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let columns: [ColumnSummary]
    public let presets: [PresetSummary]
}

public struct ColumnSummary: Codable, Identifiable, Equatable, Sendable { public let id: String; public let name: String }
public struct PresetSummary: Codable, Identifiable, Equatable, Sendable { public let id: String; public let name: String }

public struct BufferSummary: Codable, Equatable, Sendable {
    public let revision: WireRevision
    public let collecting: Bool
    public let entryCount: Int
}

public struct CardSummary: Codable, Identifiable, Equatable, Sendable {
    public let id: String
    public let projectId: String
    public let columnId: String
    public let title: String
    public let status: String
    public let generation: String?
    public let canSend: Bool
    public let canQueue: Bool
    public let buffer: BufferSummary

    private enum CodingKeys: String, CodingKey { case id, projectId, columnId, title, status, generation, canSend, canQueue, buffer }

    public init(id: String, projectId: String, columnId: String, title: String, status: String, generation: String?, canSend: Bool, canQueue: Bool, buffer: BufferSummary) {
        self.id = id; self.projectId = projectId; self.columnId = columnId; self.title = title; self.status = status
        self.generation = generation; self.canSend = canSend; self.canQueue = canQueue; self.buffer = buffer
    }

    /// The host's card status words are `running`, `stopped` and `unknown`
    /// (projection.rs `probe_status`); only `stopped` means no session.
    public static let stoppedStatuses: Set<String> = ["stopped"]
    public var isStopped: Bool { Self.stoppedStatuses.contains(status) }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        projectId = try values.decode(String.self, forKey: .projectId)
        columnId = try values.decode(String.self, forKey: .columnId)
        title = try values.decode(String.self, forKey: .title)
        status = try values.decode(String.self, forKey: .status)
        generation = try values.decodeIfPresent(String.self, forKey: .generation)
        canSend = try values.decode(Bool.self, forKey: .canSend)
        canQueue = try values.decodeIfPresent(Bool.self, forKey: .canQueue) ?? false
        buffer = try values.decode(BufferSummary.self, forKey: .buffer)
    }
}

public struct QueueItem: Codable, Identifiable, Equatable, Sendable {
    public let id: String
    public let cardId: String
    public let mode: String
    public let state: String
    public let paused: Bool
    public let revision: WireRevision
}

public struct TerminalOutput: Codable, Equatable, Sendable {
    public let cardId: String
    public let generation: String?
    public let revision: WireRevision
    public let capturedAt: WireInstant
    public let text: String
    public let truncated: Bool
}

public struct CardBuffer: Codable, Equatable, Sendable {
    public let revision: WireRevision
    public let collecting: Bool
    public let entries: [BufferEntry]
}

public struct BufferEntry: Codable, Identifiable, Equatable, Sendable {
    public let id: String
    public let kind: String
    public let text: String
    public let revision: Int
    public let createdAt: WireInstant
    public let updatedAt: WireInstant?
    public let source: BufferSource?
    public let copies: [BufferCopy]
}

public struct BufferSource: Codable, Equatable, Sendable {
    public let type: String?
    public let eventId: String?
    public let connection: String?
    public let channel: String?
    public let rule: String?
    public let at: WireInstant?
    public let links: [String]?
    public let reference: String?
}

public struct BufferCopy: Codable, Equatable, Sendable {
    public let operationId: String
    public let entryRevision: Int
    public let text: String
    public let createdAt: WireInstant
    public let state: String
}

public enum WireValidator {
    public static func validate(_ output: TerminalOutput) throws {
        guard output.text.utf8.count <= ConnectorLimits.terminalTextUTF8Bytes else { throw ConnectorError.responseTooLarge }
    }

    public static func validate(_ buffer: CardBuffer) throws {
        guard buffer.entries.count <= ConnectorLimits.bufferEntries,
              buffer.entries.reduce(0, { $0 + $1.copies.count }) <= ConnectorLimits.bufferCopies else { throw ConnectorError.responseTooLarge }
        var bytes = 0
        for entry in buffer.entries {
            let entryBytes = entry.text.utf8.count
            guard entryBytes <= ConnectorLimits.bufferEntryUTF8Bytes else { throw ConnectorError.responseTooLarge }
            bytes += entryBytes
            for copy in entry.copies {
                let copyBytes = copy.text.utf8.count
                guard copyBytes <= ConnectorLimits.bufferEntryUTF8Bytes else { throw ConnectorError.responseTooLarge }
                bytes += copyBytes
            }
        }
        guard bytes <= ConnectorLimits.bufferTotalUTF8Bytes else { throw ConnectorError.responseTooLarge }
    }

    /// Text the host would refuse is caught here, before anything is sent:
    /// the host's `command_text` rule (validate.rs) is non-empty, at most
    /// `commandTextUTF8Bytes`, and no control character except `\n` and `\t`.
    public static func validate(_ command: CommandRequest) throws {
        guard ["send-message", "buffer-add", "buffer-edit"].contains(command.kind) else { return }
        guard case let .string(text)? = command.payload["text"] else { throw ConnectorError.invalidCommandText }
        guard text.utf8.count <= ConnectorLimits.commandTextUTF8Bytes else { throw ConnectorError.requestTooLarge }
        guard !text.isEmpty, !text.unicodeScalars.contains(where: { scalar in
            scalar.properties.generalCategory == .control && scalar != "\n" && scalar != "\t"
        }) else { throw ConnectorError.invalidCommandText }
    }
}

public struct WireRevision: Codable, Equatable, Sendable, CustomStringConvertible {
    public let value: String
    public var description: String { value }

    public init(_ value: String) { self.value = value }
    public init(from decoder: Decoder) throws {
        let box = try decoder.singleValueContainer()
        if let value = try? box.decode(String.self) { self.value = value }
        else if let value = try? box.decode(Int.self) { self.value = String(value) }
        else { throw DecodingError.dataCorruptedError(in: box, debugDescription: "Revision must be a string or integer") }
    }
    public func encode(to encoder: Encoder) throws { var box = encoder.singleValueContainer(); try box.encode(value) }
}

public enum WireInstant: Codable, Equatable, Sendable, CustomStringConvertible {
    case text(String), integer(Int64), number(Double)
    public var description: String {
        switch self { case let .text(value): value; case let .integer(value): String(value); case let .number(value): String(value) }
    }
    public init(from decoder: Decoder) throws {
        let box = try decoder.singleValueContainer()
        if let value = try? box.decode(String.self) { self = .text(value) }
        else if let value = try? box.decode(Int64.self) { self = .integer(value) }
        else if let value = try? box.decode(Double.self) { self = .number(value) }
        else { throw DecodingError.dataCorruptedError(in: box, debugDescription: "Timestamp must be text or number") }
    }
    public func encode(to encoder: Encoder) throws {
        var box = encoder.singleValueContainer()
        switch self { case let .text(value): try box.encode(value); case let .integer(value): try box.encode(value); case let .number(value): try box.encode(value) }
    }
}

public struct CommandRequest: Codable, Equatable, Sendable {
    public let id: String
    public let kind: String
    public let cardId: String?
    public let expectedGeneration: String?
    public let expectedRevision: String?
    public let payload: [String: JSONValue]
    /// Admission sequence, assigned once by `CommandJournal` before the first
    /// POST and reused by every retry. The host refuses a command whose
    /// sequence is at or below its retired history, so a command can never be
    /// admitted twice even after the host dropped its record.
    public let seq: UInt64?

    public init(id: String, kind: String, cardId: String? = nil, expectedGeneration: String? = nil, expectedRevision: String? = nil, payload: [String: JSONValue], seq: UInt64? = nil) {
        self.id = id
        self.kind = kind
        self.cardId = cardId
        self.expectedGeneration = expectedGeneration
        self.expectedRevision = expectedRevision
        self.payload = payload
        self.seq = seq
    }

    /// The same command bound to `seq` (nil: the unsequenced body a caller builds).
    public func sequenced(_ seq: UInt64?) -> CommandRequest {
        CommandRequest(id: id, kind: kind, cardId: cardId, expectedGeneration: expectedGeneration, expectedRevision: expectedRevision, payload: payload, seq: seq)
    }

    private enum CodingKeys: String, CodingKey { case id, kind, cardId, expectedGeneration, expectedRevision, payload, seq }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        kind = try values.decode(String.self, forKey: .kind)
        cardId = try values.decodeIfPresent(String.self, forKey: .cardId)
        expectedGeneration = try values.decodeIfPresent(String.self, forKey: .expectedGeneration)
        expectedRevision = try values.decodeIfPresent(String.self, forKey: .expectedRevision)
        payload = try values.decode([String: JSONValue].self, forKey: .payload)
        seq = try values.decodeIfPresent(UInt64.self, forKey: .seq)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(id, forKey: .id)
        try values.encode(kind, forKey: .kind)
        try values.encodeIfPresent(cardId, forKey: .cardId)
        if let expectedGeneration { try values.encode(expectedGeneration, forKey: .expectedGeneration) }
        else { try values.encodeNil(forKey: .expectedGeneration) }
        try values.encodeIfPresent(expectedRevision, forKey: .expectedRevision)
        try values.encode(payload, forKey: .payload)
        try values.encodeIfPresent(seq, forKey: .seq)
    }
}

public enum JSONValue: Codable, Equatable, Sendable {
    case string(String), bool(Bool), strings([String]), integer(Int)

    public init(from decoder: Decoder) throws {
        let box = try decoder.singleValueContainer()
        if let value = try? box.decode(String.self) { self = .string(value) }
        else if let value = try? box.decode(Bool.self) { self = .bool(value) }
        else if let value = try? box.decode(Int.self) { self = .integer(value) }
        else if let value = try? box.decode([String].self) { self = .strings(value) }
        else { throw DecodingError.dataCorruptedError(in: box, debugDescription: "Unsupported command value") }
    }

    public func encode(to encoder: Encoder) throws {
        var box = encoder.singleValueContainer()
        switch self {
        case let .string(value): try box.encode(value)
        case let .bool(value): try box.encode(value)
        case let .strings(value): try box.encode(value)
        case let .integer(value): try box.encode(value)
        }
    }
}

public struct CommandResult: Codable, Equatable, Sendable {
    public enum State: String, Codable, Sendable { case accepted, applied, delivered, rejected, ambiguous }
    public let id: String
    public let state: State
    public let code: String?
    public let result: [String: JSONValue]?
}

public extension WireValidator {
    static func validate(_ result: CommandResult, for request: CommandRequest) throws {
        guard result.id == request.id else { throw ConnectorError.invalidResponse }
        if let code = result.code {
            // The host's rule (journal.rs `validate_terminal`): 1-64 of a-z, 0-9, '-'.
            guard code.range(of: "^[a-z0-9-]{1,64}$", options: .regularExpression) != nil else { throw ConnectorError.invalidResponse }
        }
        guard let object = result.result else { return }
        guard let encoded = try? JSONEncoder().encode(object), encoded.count <= ConnectorLimits.commandResultBytes else { throw ConnectorError.invalidResponse }
        let allowedKeys: Set<String>
        switch request.kind {
        case "task-create": allowedKeys = ["cardId"]
        case "buffer-add", "buffer-edit", "buffer-delete": allowedKeys = ["cardId", "entryId", "revision"]
        case "buffer-queue": allowedKeys = ["cardId", "revision", "queued"]
        default: allowedKeys = []
        }
        guard !allowedKeys.isEmpty else { throw ConnectorError.invalidResponse }
        guard Set(object.keys).isSubset(of: allowedKeys) else { throw ConnectorError.invalidResponse }
        for (key, value) in object {
            switch (key, value) {
            case ("cardId", .string(let value)), ("entryId", .string(let value)), ("revision", .string(let value)):
                guard !value.isEmpty, value.utf8.count <= 128 else { throw ConnectorError.invalidResponse }
            case ("queued", .integer(let value)):
                guard (0...256).contains(value) else { throw ConnectorError.invalidResponse }
            default: throw ConnectorError.invalidResponse
            }
        }
    }
}

struct ErrorEnvelope: Codable { let error: ErrorBody }
struct ErrorBody: Codable { let code: String }
