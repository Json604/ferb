import Foundation
import FerbMobile

/// How the core reaches the Mac: the real link ([LinkUplinks]), or a transport the tests supply.
public protocol Uplink: AnyObject, Sendable {
    /// Makes this the core's way to the Mac.
    func attach(_ mobile: Mobile) throws
    func pair(_ code: String) throws
    func status() -> LinkStatus?
    func setActive(_ on: Bool)
    /// The app is unlocked and in front (kb PHN-648).
    func setOpen(_ on: Bool)
    func networkChanged(_ kind: String?)
    func retry()
    func forget() throws
    /// Bounded (the link: about 5 s): not on the main thread.
    func shutdown()
}

/// What a start needs: the kept state, the names the Mac lists, the public keys for the pairing hello.
public struct UplinkStart: @unchecked Sendable {
    public let state: Data?
    public let name: String
    public let app: String
    public let keys: PublicKeys
    public let listener: LinkListener
    public let deviceSigner: DeviceSigner?
}

public extension Uplink {
    func setOpen(_ on: Bool) {}
}

public protocol UplinkFactory: Sendable {
    /// Blocking (the link: up to 20 s). Throws FerbError.
    func start(_ start: UplinkStart) throws -> Uplink
}

/// ferb-link through ferb-mobile-ffi's `Link`. `tune` lets a test point it at its own relays.
public struct LinkUplinks: UplinkFactory {
    private let tune: @Sendable (LinkConfig) -> LinkConfig

    public init(tune: @escaping @Sendable (LinkConfig) -> LinkConfig = { $0 }) {
        self.tune = tune
    }

    public func start(_ start: UplinkStart) throws -> Uplink {
        let config = tune(LinkConfig(state: start.state, name: start.name, app: start.app, unlockPub: start.keys.lock,
                                     devicePub: start.keys.device, strongPub: start.keys.strong, segmentationOffload: true,
                                     devSharedRelays: [], devForceRelay: false, devPortmapper: true))
        return LinkUplink(try Link.start(config: config, listener: start.listener, deviceSigner: start.deviceSigner))
    }
}

private final class LinkUplink: Uplink, @unchecked Sendable {
    private let link: Link
    init(_ link: Link) { self.link = link }
    func attach(_ mobile: Mobile) throws { try mobile.connect(link: link) }
    func pair(_ code: String) throws { try link.pair(code: code) }
    func status() -> LinkStatus? { link.status() }
    func setActive(_ on: Bool) { try? link.setActive(on: on) }
    func setOpen(_ on: Bool) { link.setOpen(on: on) }
    func networkChanged(_ kind: String?) { try? link.networkChanged(kind: kind) }
    func retry() { try? link.retry() }
    func forget() throws { try link.forget() }
    func shutdown() { link.shutdown() }
}
