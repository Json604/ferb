package com.ferb.core

import com.ferb.mobile.FerbException

/** A call's result: the value, or the library's typed error (sealed: one subclass per stable code). */
sealed interface Outcome<out T> {
    data class Ok<out T>(val value: T) : Outcome<T>

    data class Failed(val error: FerbException) : Outcome<Nothing> {
        val code: String get() = error.code
        val detail: String get() = error.detail
    }

    fun getOrNull(): T? = (this as? Ok)?.value
}

/** The stable code (ferb-mobile `ErrorCode::as_str`, ferb-mobile-ffi `FerbError::code`). A new code fails this build. */
val FerbException.code: String
    get() = when (this) {
        is FerbException.Closed -> "closed"
        is FerbException.BadKey -> "bad_key"
        is FerbException.WrongKey -> "wrong_key"
        is FerbException.StoreNewer -> "store_newer"
        is FerbException.Store -> "store"
        is FerbException.Offline -> "offline"
        is FerbException.NeedsBetterPath -> "needs_better_path"
        is FerbException.NeedsUnlock -> "needs_unlock"
        is FerbException.Refused -> "refused"
        is FerbException.NeedsSigner -> "needs_signer"
        is FerbException.NotSigned -> "not_signed"
        is FerbException.NotFound -> "not_found"
        is FerbException.InFlight -> "in_flight"
        is FerbException.BadReply -> "bad_reply"
        is FerbException.Cancelled -> "cancelled"
        is FerbException.BadArgument -> "bad_argument"
        is FerbException.Internal -> "internal"
    }

/** The link's own code (ferb-link errors.rs), when the failure came from there. */
val FerbException.linkCode: String?
    get() = when (this) {
        is FerbException.Closed -> linkCode
        is FerbException.BadKey -> linkCode
        is FerbException.WrongKey -> linkCode
        is FerbException.StoreNewer -> linkCode
        is FerbException.Store -> linkCode
        is FerbException.Offline -> linkCode
        is FerbException.NeedsBetterPath -> linkCode
        is FerbException.NeedsUnlock -> linkCode
        is FerbException.Refused -> linkCode
        is FerbException.NeedsSigner -> linkCode
        is FerbException.NotSigned -> linkCode
        is FerbException.NotFound -> linkCode
        is FerbException.InFlight -> linkCode
        is FerbException.BadReply -> linkCode
        is FerbException.Cancelled -> linkCode
        is FerbException.BadArgument -> linkCode
        is FerbException.Internal -> linkCode
    }

/** The words the library gave. */
val FerbException.detail: String
    get() = when (this) {
        is FerbException.Closed -> detail
        is FerbException.BadKey -> detail
        is FerbException.WrongKey -> detail
        is FerbException.StoreNewer -> detail
        is FerbException.Store -> detail
        is FerbException.Offline -> detail
        is FerbException.NeedsBetterPath -> detail
        is FerbException.NeedsUnlock -> detail
        is FerbException.Refused -> detail
        is FerbException.NeedsSigner -> detail
        is FerbException.NotSigned -> detail
        is FerbException.NotFound -> detail
        is FerbException.InFlight -> detail
        is FerbException.BadReply -> detail
        is FerbException.Cancelled -> detail
        is FerbException.BadArgument -> detail
        is FerbException.Internal -> detail
    }
