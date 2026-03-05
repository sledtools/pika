import Foundation

let microvmDogfoodWhitelistNpubs: Set<String> = [
    // justin
    "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
    // benthecarman
    "npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38",
    // Paul
    "npub1p4kg8zxukpym3h20erfa3samj00rm2gt4q5wfuyu3tg0x3jg3gesvncxf8",
]

let defaultNotificationBaseUrl = "https://test.notifs.benthecarman.com"

func isMicrovmDogfoodWhitelistedNpub(_ npub: String?) -> Bool {
    guard let normalized = npub?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
          !normalized.isEmpty else {
        return false
    }
    return microvmDogfoodWhitelistNpubs.contains(normalized)
}

struct AgentApiConfiguration: Equatable {
    let baseUrl: URL
    let signingNsec: String
}

func resolveAgentApiConfiguration(
    appConfig: [String: Any],
    env: [String: String],
    signingNsec: String?
) -> AgentApiConfiguration? {
    guard let signingNsec = signingNsec?.trimmingCharacters(in: .whitespacesAndNewlines),
          !signingNsec.isEmpty else {
        return nil
    }

    let baseUrlString = readTrimmedString(
        key: "agent_api_url",
        appConfig: appConfig,
        envKey: "PIKA_AGENT_API_URL",
        env: env
    ) ?? readTrimmedString(
        key: "notification_url",
        appConfig: appConfig,
        envKey: "PIKA_NOTIFICATION_URL",
        env: env
    ) ?? defaultNotificationBaseUrl

    guard let baseUrl = URL(string: baseUrlString) else {
        return nil
    }
    return AgentApiConfiguration(baseUrl: baseUrl, signingNsec: signingNsec)
}

private func readTrimmedString(
    key: String,
    appConfig: [String: Any],
    envKey: String,
    env: [String: String]
) -> String? {
    if let raw = appConfig[key] as? String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
    }
    if let raw = env[envKey] {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
    }
    return nil
}

enum AgentAppState: String, Decodable {
    case creating
    case ready
    case error
}

struct AgentStateResponse: Decodable {
    let agentId: String
    let vmId: String?
    let state: AgentAppState

    private enum CodingKeys: String, CodingKey {
        case agentId = "agent_id"
        case vmId = "vm_id"
        case state
    }
}

private struct AgentErrorResponse: Decodable {
    let error: String
}

enum AgentControlClientError: Error {
    case invalidRequestPath
    case invalidResponse
    case decodeFailure
    case signingFailed
    case transport(Error)
    case unauthorized
    case notWhitelisted
    case agentExists
    case agentNotFound
    case remote(String, statusCode: Int)
}

protocol AgentControlClient: AnyObject {
    func ensureAgent(config: AgentApiConfiguration) async throws -> AgentStateResponse
    func getMyAgent(config: AgentApiConfiguration) async throws -> AgentStateResponse
}

final class HttpAgentControlClient: AgentControlClient {
    private let session: URLSession

    init(session: URLSession = .shared) {
        self.session = session
    }

    func ensureAgent(config: AgentApiConfiguration) async throws -> AgentStateResponse {
        let request = try buildRequest(
            method: "POST",
            path: "/v1/agents/ensure",
            config: config
        )
        let (data, response) = try await perform(request)
        switch response.statusCode {
        case 202:
            return try decodeAgentState(from: data)
        case 401:
            throw AgentControlClientError.unauthorized
        case 403:
            throw AgentControlClientError.notWhitelisted
        case 409:
            if decodeErrorCode(from: data) == "agent_exists" {
                throw AgentControlClientError.agentExists
            }
            throw AgentControlClientError.remote(
                decodeErrorCode(from: data) ?? "agent_exists",
                statusCode: response.statusCode
            )
        default:
            throw AgentControlClientError.remote(
                decodeErrorCode(from: data) ?? "internal",
                statusCode: response.statusCode
            )
        }
    }

    func getMyAgent(config: AgentApiConfiguration) async throws -> AgentStateResponse {
        let request = try buildRequest(
            method: "GET",
            path: "/v1/agents/me",
            config: config
        )
        let (data, response) = try await perform(request)
        switch response.statusCode {
        case 200:
            return try decodeAgentState(from: data)
        case 401:
            throw AgentControlClientError.unauthorized
        case 403:
            throw AgentControlClientError.notWhitelisted
        case 404:
            if decodeErrorCode(from: data) == "agent_not_found" {
                throw AgentControlClientError.agentNotFound
            }
            throw AgentControlClientError.remote(
                decodeErrorCode(from: data) ?? "agent_not_found",
                statusCode: response.statusCode
            )
        default:
            throw AgentControlClientError.remote(
                decodeErrorCode(from: data) ?? "internal",
                statusCode: response.statusCode
            )
        }
    }

    private func perform(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                throw AgentControlClientError.invalidResponse
            }
            return (data, http)
        } catch let err as AgentControlClientError {
            throw err
        } catch {
            throw AgentControlClientError.transport(error)
        }
    }

    private func buildRequest(
        method: String,
        path: String,
        config: AgentApiConfiguration
    ) throws -> URLRequest {
        guard let url = URL(string: path, relativeTo: config.baseUrl) else {
            throw AgentControlClientError.invalidRequestPath
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        guard let authorization = buildNip98AuthorizationHeader(
            nsec: config.signingNsec,
            method: method,
            url: url.absoluteString
        ) else {
            throw AgentControlClientError.signingFailed
        }
        request.setValue(authorization, forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if method == "POST" {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = Data("{}".utf8)
        }
        return request
    }

    private func decodeAgentState(from data: Data) throws -> AgentStateResponse {
        do {
            return try JSONDecoder().decode(AgentStateResponse.self, from: data)
        } catch {
            throw AgentControlClientError.decodeFailure
        }
    }

    private func decodeErrorCode(from data: Data) -> String? {
        guard let payload = try? JSONDecoder().decode(AgentErrorResponse.self, from: data) else {
            return nil
        }
        let normalized = payload.error.trimmingCharacters(in: .whitespacesAndNewlines)
        return normalized.isEmpty ? nil : normalized
    }
}
