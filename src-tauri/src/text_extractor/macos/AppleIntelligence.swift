import Foundation
import FoundationModels
import ImageIO
import Security

// Compiled only with the macOS 27 SDK. Every entry point remains safe on older
// systems; the framework itself is weak-linked into the Pulse binary.
private struct AppleRequest: Decodable, Sendable {
    let operation: String
    let instructions: String?
    let text: String?
    let imageBase64: String?
}
private struct Availability: Encodable {
    let available: Bool
    let checking = false
    let message: String?
}
private struct ProviderError: Error { let message: String }
private final class Requests: @unchecked Sendable {
    static let shared = Requests()
    private let lock = NSLock()
    private var tasks: [UInt64: Task<Void, Never>] = [:]
    func start(_ id: UInt64, work: @escaping @Sendable () async -> Void) {
        lock.lock()
        tasks[id] = Task.detached {
            await work()
            self.remove(id)
        }
        lock.unlock()
    }
    private func remove(_ id: UInt64) {
        lock.lock(); defer { lock.unlock() }
        tasks.removeValue(forKey: id)
    }
    func cancel(_ id: UInt64) {
        lock.lock()
        let task = tasks.removeValue(forKey: id)
        lock.unlock()
        task?.cancel()
    }
}

private func hasCloudEntitlement() -> Bool {
    var code: SecCode?
    var staticCode: SecStaticCode?
    var information: CFDictionary?
    guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
          SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
          SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
          let dictionary = information as? [String: Any],
          let entitlements = dictionary[kSecCodeInfoEntitlementsDict as String] as? [String: Any]
    else { return false }
    return entitlements["com.apple.developer.private-cloud-compute"] as? Bool == true
}

@available(macOS 27.0, *)
private func availability(_ model: PrivateCloudComputeLanguageModel) -> Availability {
    guard hasCloudEntitlement() else {
        return Availability(available: false, message: "Access denied. This Pulse build needs Apple's Private Cloud Compute permission.")
    }
    switch model.availability {
    case .available:
        if model.quotaUsage.isLimitReached {
            return Availability(available: false, message: "Apple Intelligence's daily limit has been reached. Retry later or select OpenAI.")
        }
        return Availability(available: true, message: nil)
    case .unavailable(.deviceNotEligible):
        return Availability(available: false, message: "Apple Intelligence is not available on this Mac or in this region.")
    case .unavailable(.systemNotReady):
        return Availability(available: false, message: "Apple Intelligence is not ready. Enable it in System Settings, then retry.")
    case .unavailable:
        return Availability(available: false, message: "Apple Intelligence is unavailable. Check System Settings, then retry.")
    }
}

@available(macOS 27.0, *)
private func run(_ request: AppleRequest) async throws -> String {
    // Check signing before initializing a managed service in an ordinary dev build.
    guard hasCloudEntitlement() else {
        let denied = Availability(available: false, message: "Access denied. This Pulse build needs Apple's Private Cloud Compute permission.")
        if request.operation == "status" { return String(decoding: try JSONEncoder().encode(denied), as: UTF8.self) }
        throw ProviderError(message: denied.message!)
    }
    let model = PrivateCloudComputeLanguageModel()
    let status = availability(model)
    if request.operation == "status" {
        return String(decoding: try JSONEncoder().encode(status), as: UTF8.self)
    }
    guard status.available else { throw ProviderError(message: status.message ?? "Apple Intelligence is unavailable.") }
    try Task.checkCancellation()
    let session = LanguageModelSession(model: model, instructions: request.instructions ?? "")
    let options = GenerationOptions(samplingMode: .greedy, maximumResponseTokens: 16384)
    let context = ContextOptions(reasoningLevel: .light)
    if let encoded = request.imageBase64 {
        guard let bytes = Data(base64Encoded: encoded),
              let source = CGImageSourceCreateWithData(bytes as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else { throw ProviderError(message: "Could not prepare this selection for Apple Intelligence.") }
        let response = try await session.respond(options: options, contextOptions: context) {
            request.text ?? "Extract the main text from this selection."
            Attachment(image)
        }
        try Task.checkCancellation()
        return response.content
    }
    let response = try await session.respond(to: request.text ?? "", options: options, contextOptions: context)
    try Task.checkCancellation()
    return response.content
}

private func message(for error: Error) -> String {
    if error is CancellationError { return "Capture cancelled." }
    if let error = error as? ProviderError { return error.message }
    if #available(macOS 27.0, *) {
        if let error = error as? PrivateCloudComputeLanguageModel.Error {
            switch error {
            case .networkFailure: return "Could not reach Apple Intelligence. Check your connection and retry."
            case .quotaLimitReached: return "Apple Intelligence's daily limit has been reached. Retry later or select OpenAI."
            case .serviceUnavailable: return "Apple Intelligence is temporarily unavailable. Retry later or select OpenAI."
            @unknown default: break
            }
        }
        if let error = error as? LanguageModelError {
            switch error {
            case .unsupportedLanguageOrLocale: return "Apple Intelligence does not support this language. Select OpenAI or use Basic."
            case .contextSizeExceeded: return "This selection is too large for Apple Intelligence. Select a smaller area."
            case .guardrailViolation, .refusal: return "Apple Intelligence could not process this selection. Try Basic or select OpenAI."
            default: break
            }
        }
    }
    // Framework diagnostics can contain prompts. Never expose raw error details.
    return "Apple Intelligence could not complete the request. Check access and retry, or select OpenAI."
}

@_cdecl("pulse_apple_start")
func pulseAppleStart(_ id: UInt64, _ json: UnsafePointer<CChar>, _ reply: @escaping @convention(c) (UInt64, Int32, UnsafePointer<CChar>?) -> Void) {
    let data = Data(String(cString: json).utf8)
    Requests.shared.start(id) {
        do {
            let request = try JSONDecoder().decode(AppleRequest.self, from: data)
            let result: String
            if #available(macOS 27.0, *) {
                result = try await run(request)
            } else {
                let status = Availability(available: false, message: "Apple's cloud models require macOS 27 or later. Use OpenAI or Basic on this Mac.")
                if request.operation == "status" {
                    result = String(decoding: try JSONEncoder().encode(status), as: UTF8.self)
                } else { throw ProviderError(message: status.message!) }
            }
            result.withCString { reply(id, 1, $0) }
        } catch {
            message(for: error).withCString { reply(id, 0, $0) }
        }
    }
}

@_cdecl("pulse_apple_cancel")
func pulseAppleCancel(_ id: UInt64) { Requests.shared.cancel(id) }
