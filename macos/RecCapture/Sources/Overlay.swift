import AppKit
import Foundation

final class OverlayState: @unchecked Sendable {
    let lock = NSLock()
    var runId = "----"
    var step: UInt32 = 0
    var action: String?
    var verdict: String?
    var cardKind: String?
    var cardBody: String?
    var cardUntil: Date?
    var gitTitle: String?
    var gitRepo: String?
    var gitBranch: String?
    var gitCommit: String?
    var gitTree: String?
    var gitUntil: Date?
    var targetLost = false

    func snapshot() -> OverlayState {
        lock.lock(); defer { lock.unlock() }
        let copy = OverlayState()
        copy.runId = runId; copy.step = step; copy.action = action; copy.verdict = verdict
        copy.cardKind = cardKind; copy.cardBody = cardBody; copy.cardUntil = cardUntil
        copy.gitTitle = gitTitle; copy.gitRepo = gitRepo; copy.gitBranch = gitBranch
        copy.gitCommit = gitCommit; copy.gitTree = gitTree; copy.gitUntil = gitUntil
        copy.targetLost = targetLost
        return copy
    }
    func apply(json: [String: Any], now: Date = Date()) {
        lock.lock(); defer { lock.unlock() }
        if let run = json["run_id"] as? String { runId = run }
        if let step = json["step"] as? NSNumber { self.step = step.uint32Value }
        switch json["cmd"] as? String {
        case "hud":
            if let text = json["action"] as? String { action = text }
            if json["verdict"] is NSNull { verdict = nil }
            else if let text = json["verdict"] as? String { verdict = text }
        case "card":
            cardKind = json["kind"] as? String; cardBody = json["body"] as? String
            cardUntil = now.addingTimeInterval(4)
        case "git":
            gitTitle = json["title"] as? String; gitRepo = json["repository"] as? String
            gitBranch = json["branch"] as? String; gitCommit = json["commit"] as? String
            gitTree = json["working_tree"] as? String; gitUntil = now.addingTimeInterval(4.5)
        case "target_lost": targetLost = true
        default: break
        }
    }
    /// Read a complete Cua snapshot. Absolute expiry avoids renewing cards on each still frame.
    func loadSnapshot(_ json: [String: Any]) {
        apply(json: json)
        lock.lock(); defer { lock.unlock() }
        action = json["action"] as? String; verdict = json["verdict"] as? String
        cardKind = json["kind"] as? String; cardBody = json["body"] as? String
        gitTitle = json["title"] as? String; gitRepo = json["repository"] as? String
        gitBranch = json["branch"] as? String; gitCommit = json["commit"] as? String
        gitTree = json["working_tree"] as? String
        if let t = json["card_until_ms"] as? NSNumber { cardUntil = Date(timeIntervalSince1970: t.doubleValue / 1000) }
        if let t = json["git_until_ms"] as? NSNumber { gitUntil = Date(timeIntervalSince1970: t.doubleValue / 1000) }
        if let lost = json["target_lost"] as? Bool { targetLost = lost }
    }
    func cardVisible(at time: Date) -> Bool { cardUntil.map { $0 > time } ?? false }
    func gitVisible(at time: Date) -> Bool { gitUntil.map { $0 > time } ?? false }
}

func wrapped(_ text: String, font: NSFont, width: CGFloat, limit: Int) -> [String] {
    var rows: [String] = []
    for raw in text.components(separatedBy: "\n") {
        var line = ""
        // Character wrapping also supports Korean and long command arguments without spaces.
        for character in raw {
            let next = line + String(character)
            if !line.isEmpty && (next as NSString).size(withAttributes: [.font: font]).width > width {
                rows.append(line); line = String(character)
            } else { line = next }
        }
        rows.append(line)
    }
    if rows.count > limit { return Array(rows.prefix(max(0, limit - 1))) + [String(rows[limit - 1].prefix(60)) + "…"] }
    return rows
}

func drawOverlay(size: NSSize, state: OverlayState) {
    // Scale with the encoded frame so Retina captures remain readable on playback.
    let scale = max(0.5, min(size.width / 1280, size.height / 720))
    let pad = 16 * scale
    let idFont = NSFont.monospacedSystemFont(ofSize: 32 * scale, weight: .bold)
    let bodyFont = NSFont.systemFont(ofSize: 26 * scale, weight: .medium)
    let metaFont = NSFont.monospacedSystemFont(ofSize: 22 * scale, weight: .medium)
    let lineH = 40 * scale
    let width = max(1, size.width - pad * 2)
    let now = Date()
    var body: [String] = []
    if let verdict = state.verdict { body.append(verdict) }
    if state.cardVisible(at: now), let text = state.cardBody {
        body.append(state.cardKind ?? "EVENT")
        body += wrapped(text, font: bodyFont, width: width, limit: 8)
    } else if let action = state.action {
        body += wrapped(action, font: bodyFont, width: width, limit: 2)
    }
    var context: [String] = []
    if state.gitVisible(at: now) {
        if let repo = state.gitRepo {
            context = wrapped("Git: \(repo) | \(state.gitBranch ?? "-") | \(state.gitCommit ?? "-") | \(state.gitTree ?? "-")", font: metaFont, width: width, limit: 2)
        } else { context = ["Git: unavailable"] }
    }
    if state.targetLost {
        body.insert(contentsOf: wrapped("CAPTURE TARGET LOST — last frame retained", font: bodyFont, width: width, limit: 2), at: 0)
    }
    // Bound the panel and truncate complete rows instead of drawing off-frame.
    let rowBudget = max(1, Int((size.height * 0.5 - pad * 2) / lineH))
    context = Array(context.prefix(max(0, rowBudget - 1)))
    let bodyBudget = max(0, rowBudget - 1 - context.count)
    if body.count > bodyBudget {
        body = Array(body.prefix(bodyBudget))
        if !body.isEmpty { body[body.count - 1] = "…" }
    }
    let height = min(size.height, pad * 2 + lineH * CGFloat(1 + body.count + context.count))
    let top = size.height - height
    NSColor(white: 0.05, alpha: 0.94).setFill()
    NSRect(x: 0, y: top, width: size.width, height: height).fill()
    var y = top + pad
    let identifier = String(format: "%@:%03u", state.runId, state.step)
    (identifier as NSString).draw(at: NSPoint(x: pad, y: y), withAttributes: [.font: idFont, .foregroundColor: NSColor.white])
    for line in body {
        y += lineH
        if y + lineH > size.height { break }
        (line as NSString).draw(at: NSPoint(x: pad, y: y), withAttributes: [.font: bodyFont, .foregroundColor: NSColor.white])
    }
    for line in context {
        y += lineH
        if y + lineH > size.height { break }
        (line as NSString).draw(at: NSPoint(x: pad, y: y), withAttributes: [.font: metaFont, .foregroundColor: NSColor(white: 0.8, alpha: 1)])
    }
}
