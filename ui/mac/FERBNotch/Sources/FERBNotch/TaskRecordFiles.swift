import Foundation

/// Full trace exports stay out of SwiftUI's text layout and in this process's private temp folder.
final class TaskRecordFiles: @unchecked Sendable {
    private let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("FERBNotch-TaskRecords-" + UUID().uuidString, isDirectory: true)

    func write(_ trace: [String: Any]) async throws -> URL {
        let job = Task.detached(priority: .userInitiated) { [directory] in
            try Task.checkCancellation()
            let data = try JSONSerialization.data(withJSONObject: trace,
                options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
            try Task.checkCancellation()
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            let url = directory.appendingPathComponent("Full task record " + UUID().uuidString + ".json")
            do {
                try data.write(to: url, options: .atomic)
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
                try Task.checkCancellation()
                return url
            } catch {
                try? FileManager.default.removeItem(at: url)
                throw error
            }
        }
        return try await withTaskCancellationHandler { try await job.value } onCancel: { job.cancel() }
    }

    func discard(_ url: URL) {
        guard url.deletingLastPathComponent() == directory else { return }
        Task.detached { try? FileManager.default.removeItem(at: url) }
    }

    deinit {
        let directory = directory
        Task.detached { try? FileManager.default.removeItem(at: directory) }
    }
}
