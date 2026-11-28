#if FERB_PLAYGROUND
import Foundation

/// Settings → Phone in the playground (T-1205): the daemon's `phone.*` and the phone link's `link.*` answered with two
/// invented phones, a pairing code that a sample phone answers, and a relay that only checks its address's shape.
/// Nothing reaches a phone, a relay or the network.
@MainActor
final class PlaygroundPhone {
    var phones: [[String: Any]] = [
        ["id": "sample-pixel", "name": "Sample Pixel", "app": "android", "paired_at": PlaygroundSamples.time(-9 * 86_400),
         "last_seen": PlaygroundSamples.time(-300), "unused_warning": false, "fingerprint_key": true],
        ["id": "sample-tablet", "name": "Sample tablet", "app": "android", "paired_at": PlaygroundSamples.time(-50 * 86_400),
         "last_seen": PlaygroundSamples.time(-35 * 86_400), "unused_warning": true]]
    var pairing: [String: Any]?
    var ownRelay: String?
    var always = false
    var audit: [[String: Any]] = [
        ["at": PlaygroundSamples.time(-900), "phone": "sample-pixel", "method": "chat.send", "class": "plain", "outcome": "ok"],
        ["at": PlaygroundSamples.time(-600), "phone": "sample-pixel", "method": "chat.approve", "class": "private", "outcome": "ok"]]
    /// A sample QR (a finder square in each corner), so the page draws one at its real size.
    private static let rows: [String] = (0..<25).map { y in
        String((0..<25).map { x -> Character in
            let corner = (x < 7 && y < 7) || (x > 17 && y < 7) || (x < 7 && y > 17)
            return corner ? ((x % 6 == 0 || y % 6 == 0 || (x % 18 > 1 && x % 18 < 5 && y % 18 > 1 && y % 18 < 5)) ? "1" : "0")
                : ((x * 7 + y * 3) % 5 < 2 ? "1" : "0")
        })
    }

    func answer(_ method: String, _ params: [String: Any]) -> [String: Any]? {
        switch method {
        case "phone.list":
            return ["phones": phones, "pairing": pairing ?? NSNull(), "max": 5, "unused_days": 30]
        case "phone.rename":
            let id = params["id"] as? String, name = params["name"] as? String ?? ""
            phones = phones.map { $0["id"] as? String == id ? $0.merging(["name": name]) { $1 } : $0 }
            return ["id": id ?? "", "name": name]
        case "phone.remove":
            let id = params["id"] as? String
            phones.removeAll { $0["id"] as? String == id }
            audit.append(["at": PlaygroundSamples.time(), "phone": "", "method": "phone.remove", "class": "owner", "outcome": "ok"])
            return ["removed": id ?? ""]
        case "phone.pair_decide":
            guard let waiting = pairing else { return nil }
            pairing = nil
            let yes = params["yes"] as? Bool == true
            let update = waiting["update"] as? Bool == true
            if yes, !update {
                phones.append(["id": waiting["phone"] ?? "", "name": waiting["name"] ?? "", "app": "android",
                               "paired_at": PlaygroundSamples.time(), "last_seen": PlaygroundSamples.time(), "unused_warning": false])
            }
            audit.append(["at": PlaygroundSamples.time(), "phone": "", "method": "phone.pair_decide", "class": "owner",
                          "outcome": yes ? "ok" : "denied"])
            return ["phone": waiting["phone"] ?? "", "name": waiting["name"] ?? "", "paired": yes, "update": update]
        case "phone.audit": return ["rows": audit]
        case "phone.link_status":
            let tier = ownRelay == nil ? "direct" : "own_relay"
            let devices: [[String: Any]] = phones.map { row in
                let id = row["id"] as? String ?? ""
                return ["id": id, "online": id == "sample-pixel", "tier": tier]
            }
            let relay: [String: Any] = ownRelay == nil ? ["state": "shared", "connected": true] : ["state": "own"]
            var status: [String: Any] = ["relay": relay, "own_relay": NSNull(), "own_relay_always": always, "devices": devices]
            if let ownRelay { status["own_relay"] = ["url": ownRelay, "token_set": true] as [String: Any] }
            return status
        case "phone.pair_start":
            // a sample phone scans it a few seconds later: a new phone, or the paired phone the update names
            let update = (params["update"] as? String).flatMap { id in phones.first { $0["id"] as? String == id } }
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(4))
                if let update {
                    self?.pairing = ["request_id": "sample-update", "phone": update["id"] ?? "", "name": update["name"] ?? "",
                                     "app": "android", "code": "305 718", "seconds_left": 120, "update": true]
                } else {
                    self?.pairing = ["request_id": "sample-request", "phone": "sample-new-phone", "name": "Sample new phone",
                                     "app": "android", "code": "482 913", "seconds_left": 120]
                }
            }
            return ["code": "FERB1-SAMPLE-CODE-ONLY", "url": "ferb://pair?sample", "qr": Self.rows, "qr_old": Self.rows, "seconds": 120]
        case "phone.pair_cancel": pairing = nil; return ["ok": true]
        case "phone.relay_set" where params["off"] as? Bool == true:
            ownRelay = nil
            always = false
            return ["saved": true, "test": NSNull()]
        case "phone.relay_test", "phone.relay_set":
            let url = params["url"] as? String ?? ""
            guard url.hasPrefix("ferbrelay://") || url.hasPrefix("https://") else {
                return nil
            }
            let test: [String: Any] = ["url": url, "ms": 40, "steps": ["Sample relay answered (playground: nothing was contacted)"],
                                       "warnings": []]
            if method == "phone.relay_test" { return test }
            ownRelay = url.hasPrefix("ferbrelay://") ? "https://relay.example.com" : url
            return ["saved": true, "test": test]
        case "phone.relay_always": always = params["on"] as? Bool ?? always; return ["own_relay_always": always]
        default: return nil
        }
    }
}
#endif
