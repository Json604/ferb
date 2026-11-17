package com.ferb.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.app.UiState
import com.ferb.design.components.Connection
import com.ferb.design.components.LinkMark
import com.ferb.design.components.PhoneTab
import com.ferb.design.preview.labRelay
import com.ferb.design.screens.ConnectionSheet
import com.ferb.design.screens.PhoneInsets
import com.ferb.mobile.LinkPath
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkStatus

// The connection control in the top bar and its sheet (kb PHN-751, PHN-752): the one place the link's state is read
// for them. Connecting is never an error: "can't reach" waits for [UiState.linkLost] (PHN-753).

/** The link's mark: connected by its path, else connecting until the Mac has really been out of reach. */
fun linkMark(l: LinkStatus?, lost: Boolean): LinkMark = when {
    l?.phase == LinkPhase.ONLINE -> when (l.path) {
        LinkPath.DIRECT -> LinkMark.DIRECT
        LinkPath.OWN_RELAY -> LinkMark.OWN_RELAY
        else -> LinkMark.SHARED_RELAY
    }
    lost -> LinkMark.UNREACHABLE
    else -> LinkMark.CONNECTING
}

/** How the link stands, in his words. */
@Composable
fun linkStateWords(mark: LinkMark): String = stringResource(
    when (mark) {
        LinkMark.CONNECTING -> R.string.conn_connecting
        LinkMark.UNREACHABLE -> R.string.conn_unreachable
        else -> R.string.conn_connected
    },
)

/** The path, in his words, when connected. */
@Composable
fun linkPathWords(mark: LinkMark): String? = when (mark) {
    LinkMark.DIRECT -> stringResource(R.string.conn_direct)
    LinkMark.OWN_RELAY -> stringResource(R.string.mac_own_relay)
    LinkMark.SHARED_RELAY -> stringResource(R.string.conn_shared)
    else -> null
}

/** The control's value: its mark, what a screen reader says, and its tap (the sheet). */
@Composable
fun rememberConnection(vm: FerbViewModel, ui: UiState): Connection {
    val mark = linkMark(ui.link, ui.linkLost)
    val words = listOfNotNull(linkStateWords(mark), linkPathWords(mark)).joinToString(", ")
    return Connection(mark, stringResource(R.string.conn_a11y, words)) { vm.show(Sheet.Connection) }
}

/**
 * The connection sheet: the Mac's name, the state, the path and (on the shared relay) what works there, the link's own
 * words for why it can't connect, Try again while not connected, and the Mac tab for the rest (which keeps the relay
 * settings and the rest of what it already shows).
 */
@Composable
fun ConnectionSheetFor(vm: FerbViewModel, ui: UiState, insets: PhoneInsets, close: () -> Unit) {
    val mark = linkMark(ui.link, ui.linkLost)
    val online = ui.link?.phase == LinkPhase.ONLINE
    ConnectionSheet(
        mac = ui.mac?.name ?: stringResource(R.string.mac_name),
        mark = mark,
        state = linkStateWords(mark),
        path = linkPathWords(mark),
        reason = if (mark == LinkMark.UNREACHABLE) ui.link?.problem?.detail else null,
        relay = if (mark == LinkMark.SHARED_RELAY) labRelay else null,
        tryAgain = if (online) null else stringResource(R.string.try_again),
        onTryAgain = { vm.retryLink() },
        more = stringResource(R.string.conn_more),
        onMore = { close(); vm.choose(PhoneTab.MAC) },
        insets = insets,
        onClose = close,
    )
}
