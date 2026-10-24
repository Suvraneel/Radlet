import Foundation
import RadletKit

enum EngineError: LocalizedError {
    case helperMissing
    case helperStopped
    case remote(code: String, message: String)

    var errorDescription: String? {
        switch self {
        case .helperMissing: return "Radlet's helper is missing from the app. Reinstall Radlet to put it back."
        case .helperStopped: return "Radlet's helper stopped unexpectedly. It restarts on its own, so try again."
        case .remote(_, let message): return message
        }
    }
}

/// Talks to the bundled `radlet serve` helper over JSON lines.
///
/// All state lives on one serial queue. Replies resume async callers;
/// job events are delivered on the main queue. If the helper dies, pending
/// requests fail, running jobs report as cancelled, and the next request
/// starts a fresh helper.
final class EngineClient: @unchecked Sendable {
    static let shared = EngineClient()

    private let queue = DispatchQueue(label: "com.usemonoma.radlet.engine")
    private var process: Process?
    private var input: FileHandle?
    private var lines = LineBuffer()
    private var nextID = 1
    /// Increases each time a helper starts, so a job registered after its
    /// helper died can be told it ended.
    private var helperGeneration = 0
    private var pending: [Int: (Result<JSONValue, Error>) -> Void] = [:]
    private var jobHandlers: [Int: (JobEvent) -> Void] = [:]
    /// Events that arrived before their job's handler was registered.
    private var earlyEvents: [Int: [JSONValue]] = [:]

    private init() {}

    /// The helper inside the app bundle, or `RADLET_HELPER` during development.
    private var helperURL: URL {
        if let override = ProcessInfo.processInfo.environment["RADLET_HELPER"] {
            return URL(fileURLWithPath: override)
        }
        return Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/radlet")
    }

    private func startIfNeeded() throws {
        if let p = process {
            if p.isRunning { return }
            // It died but its exit hasn't been handled yet: fail its pending
            // requests and jobs now, before a new helper takes its place.
            helperExited(p)
        }
        let url = helperURL
        guard FileManager.default.isExecutableFile(atPath: url.path) else {
            Log.engine("helper missing at \(url.lastPathComponent)")
            throw EngineError.helperMissing
        }
        let p = Process()
        p.executableURL = url
        p.arguments = ["serve"]
        let stdinPipe = Pipe()
        let stdoutPipe = Pipe()
        p.standardInput = stdinPipe
        p.standardOutput = stdoutPipe
        p.standardError = FileHandle.standardError
        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            self.queue.async {
                if data.isEmpty {
                    handle.readabilityHandler = nil
                } else {
                    self.consume(data)
                }
            }
        }
        p.terminationHandler = { [weak self] proc in
            guard let self else { return }
            self.queue.async { self.helperExited(proc) }
        }
        try p.run()
        helperGeneration += 1
        process = p
        input = stdinPipe.fileHandleForWriting
        lines.reset()
        Log.engine("helper started (pid \(p.processIdentifier))")
    }

    private func helperExited(_ proc: Process) {
        guard proc === process else { return }
        Log.engine("helper exited (status \(proc.terminationStatus), reason \(proc.terminationReason.rawValue)); "
                   + "failing \(pending.count) requests and \(jobHandlers.count) jobs")
        process = nil
        input = nil
        let failed = pending
        pending.removeAll()
        failed.values.forEach { $0(.failure(EngineError.helperStopped)) }
        let jobs = jobHandlers
        jobHandlers.removeAll()
        earlyEvents.removeAll()
        jobs.values.forEach { $0(.jobFinished(.zero, cancelled: true)) }
    }

    private func consume(_ data: Data) {
        for line in lines.append(data) {
            guard let msg = try? JSONValue.parse(line) else {
                Log.engine("ignored a line that isn't JSON (\(line.count) bytes)")
                continue
            }
            route(msg)
        }
    }

    private func route(_ msg: JSONValue) {
        if let id = msg["id"]?.int {
            guard let callback = pending.removeValue(forKey: id) else { return }
            if let err = msg["error"] {
                let code = err["code"]?.string ?? "error"
                let message = err["message"]?.string ?? "Unknown error"
                Log.engine("request \(id) failed: \(code)")
                callback(.failure(EngineError.remote(code: code, message: message)))
            } else {
                callback(.success(msg["result"] ?? .null))
            }
            return
        }
        guard let job = msg["job"]?.int else { return }
        if let handler = jobHandlers[job] {
            deliver(msg, job: job, to: handler)
        } else {
            earlyEvents[job, default: []].append(msg)
        }
    }

    private func deliver(_ msg: JSONValue, job: Int, to handler: @escaping (JobEvent) -> Void) {
        guard let event = JobEvent(message: msg) else { return }
        if event.isFinal {
            jobHandlers.removeValue(forKey: job)
        }
        handler(event)
    }

    /// Sends one request and waits for its reply.
    func request(_ method: String, _ params: [String: JSONValue] = [:]) async throws -> JSONValue {
        try await requestWithGeneration(method, params).value
    }

    /// Like `request`, also returning which helper answered.
    private func requestWithGeneration(_ method: String, _ params: [String: JSONValue])
        async throws -> (value: JSONValue, generation: Int) {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<(value: JSONValue, generation: Int), Error>) in
            queue.async {
                do {
                    try self.startIfNeeded()
                    let id = self.nextID
                    self.nextID += 1
                    let generation = self.helperGeneration
                    self.pending[id] = { result in cont.resume(with: result.map { (value: $0, generation: generation) }) }
                    let req: JSONValue = .object([
                        "id": .int(Int64(id)),
                        "method": .string(method),
                        "params": .object(params),
                    ])
                    do {
                        try self.input?.write(contentsOf: try req.line())
                    } catch {
                        self.pending.removeValue(forKey: id)
                        Log.engine("couldn't send \(method): \(error.localizedDescription)")
                        throw error
                    }
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }
    }

    func hello() async throws -> Hello {
        try await request("hello").decode(Hello.self)
    }

    func inspect(_ files: [URL], recipesDir: URL?) async throws -> InspectResult {
        var params: [String: JSONValue] = ["paths": .array(files.map { .string($0.path) })]
        if let recipesDir { params["recipes_dir"] = .string(recipesDir.path) }
        return try await request("inspect", params).decode(InspectResult.self)
    }

    /// Starts a job. `onEvent` runs on the main queue for every event,
    /// ending with `.jobFinished`.
    func run(files: [URL], action: JSONValue, output: JSONValue, isolate: Bool,
             onEvent: @escaping @MainActor (JobEvent) -> Void) async throws -> Int {
        let (result, generation) = try await requestWithGeneration("run", [
            "paths": .array(files.map { .string($0.path) }),
            "action": action,
            "options": .object(["output": output]),
            "isolate": .bool(isolate),
        ])
        guard let job = result["job"]?.int else { throw EngineError.helperStopped }
        let mainHandler: (JobEvent) -> Void = { event in
            DispatchQueue.main.async { MainActor.assumeIsolated { onEvent(event) } }
        }
        queue.async {
            // The helper that accepted this job died before we got here: its
            // events are gone, so end the job now instead of waiting forever.
            guard self.helperGeneration == generation, self.process != nil else {
                mainHandler(.jobFinished(.zero, cancelled: true))
                return
            }
            let early = self.earlyEvents.removeValue(forKey: job) ?? []
            self.jobHandlers[job] = mainHandler
            for msg in early {
                // The job may finish while replaying; deliver() unregisters it.
                self.deliver(msg, job: job, to: mainHandler)
            }
        }
        return job
    }

    func cancel(job: Int) {
        Task { _ = try? await request("cancel", ["job": .int(Int64(job))]) }
    }
}
