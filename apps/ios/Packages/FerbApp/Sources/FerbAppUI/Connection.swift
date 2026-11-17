import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI

// The connection control in the top bar and its sheet (kb PHN-751, PHN-752; Android's ui/Connection.kt): the one place
// the link's state is read for them. Connecting is never an error: "can't reach" waits for UiState.linkLost (PHN-753).

/// The link's mark: connected by its path, else connecting until the Mac has really been out of reach.
func linkMark(_ l: LinkStatus?, lost: Bool) -> LinkMark {
    if l?.phase == .online {
        switch l?.path {
        case .direct: return .direct
        case .ownRelay: return .ownRelay
        default: return .sharedRelay
        }
    }
    return lost ? .unreachable : .connecting
}

/// How the link stands, in his words.
func linkStateWords(_ mark: LinkMark) -> String {
    switch mark {
    case .connecting: L.s("conn_connecting")
    case .unreachable: L.s("conn_unreachable")
    default: L.s("conn_connected")
    }
}

/// The path, in his words, when connected.
func linkPathWords(_ mark: LinkMark) -> String? {
    switch mark {
    case .direct: L.s("conn_direct")
    case .ownRelay: L.s("mac_own_relay")
    case .sharedRelay: L.s("conn_shared")
    default: nil
    }
}

/// The control's value: its mark, what VoiceOver says, and its tap (the sheet).
@MainActor
func connection(_ model: AppModel) -> Connection {
    let mark = linkMark(model.ui.link, lost: model.ui.linkLost)
    let words = [linkStateWords(mark), linkPathWords(mark)].compactMap { $0 }.joined(separator: ", ")
    return Connection(mark: mark, label: L.s("conn_a11y", words)) { model.show(.connection) }
}

/// The connection sheet: the Mac's name, the state, the path and (on the shared relay) what works there, the link's
/// own words for why it can't connect, Try again while not connected, and the Mac tab for the rest.
struct ConnectionSheetFor: View {
    @Bindable var model: AppModel
    let insets: PhoneInsets
    let close: () -> Void

    var body: some View {
        let mark = linkMark(model.ui.link, lost: model.ui.linkLost)
        let online = model.ui.link?.phase == .online
        ConnectionSheet(mac: model.ui.mac?.name ?? L.s("mac_name"), mark: mark, state: linkStateWords(mark), path: linkPathWords(mark),
                        reason: mark == .unreachable ? model.ui.link?.problem?.detail : nil,
                        relay: mark == .sharedRelay ? LabSample.relay : nil,
                        tryAgain: online ? nil : L.s("try_again"), onTryAgain: { model.retryLink() },
                        more: L.s("conn_more"), onMore: { close(); model.choose(.mac) }, insets: insets, onClose: close)
    }
}
