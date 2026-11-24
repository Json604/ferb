package com.ferb.core

import com.ferb.mobile.DeviceSigner
import com.ferb.mobile.Link
import com.ferb.mobile.LinkConfig
import com.ferb.mobile.LinkListener
import com.ferb.mobile.LinkStatus
import com.ferb.mobile.Mobile

/** How the core reaches the Mac: the real link ([LinkUplinks]), or a transport the tests supply. */
interface Uplink {
    /** Makes this the core's way to the Mac. */
    fun attach(mobile: Mobile)

    /**
     * Pairs from a scanned or typed code. [account]: the server's signed statement of this phone's FERB account for
     * this link's own [endpointId] (`/api/app/assertion`), which the Mac's daemon checks before it asks (kb PHN-828).
     */
    fun pair(code: String, account: String)

    /** This phone's link id (its endpoint id): what the account statement names; empty when the link can't say. */
    fun endpointId(): String
    fun status(): LinkStatus?
    fun setActive(on: Boolean)

    /** The app is unlocked and in front (kb PHN-648). */
    fun setOpen(on: Boolean) {}
    fun networkChanged(kind: String?)
    fun retry()
    fun forget()

    /** Bounded (the link: about 5 s): not on the main thread. */
    fun shutdown()

    /** Frees the native object (after [shutdown]). */
    fun close()
}

/** What a start needs: the kept state, the names the Mac lists, the public keys for the pairing hello. */
class UplinkStart(
    val state: ByteArray?,
    val name: String,
    val app: String,
    val keys: PublicKeys,
    val listener: LinkListener,
    val deviceSigner: DeviceSigner?,
)

fun interface UplinkFactory {
    /** Blocking (the link: up to 20 s). @throws com.ferb.mobile.FerbException */
    fun start(start: UplinkStart): Uplink
}

/** ferb-link through ferb-mobile-ffi's `Link`. [tune] lets a test point it at its own relays. */
class LinkUplinks(private val tune: (LinkConfig) -> LinkConfig = { it }) : UplinkFactory {
    override fun start(start: UplinkStart): Uplink {
        val config = tune(
            LinkConfig(
                state = start.state,
                name = start.name,
                app = start.app,
                unlockPub = start.keys.lock,
                devicePub = start.keys.device,
                strongPub = start.keys.strong,
            ),
        )
        return LinkUplink(Link.start(config, start.listener, start.deviceSigner))
    }
}

private class LinkUplink(private val link: Link) : Uplink {
    override fun attach(mobile: Mobile) = mobile.connect(link)
    override fun pair(code: String, account: String) = link.pair(code, account)
    override fun endpointId(): String = link.endpointId()
    override fun status(): LinkStatus? = link.status()
    override fun setActive(on: Boolean) = link.setActive(on)
    override fun networkChanged(kind: String?) = link.networkChanged(kind)
    override fun retry() = link.retry()
    override fun forget() = link.forget()
    override fun setOpen(on: Boolean) = link.setOpen(on)
    override fun shutdown() = link.shutdown()
    override fun close() = link.close()
}
