package com.ferb.app.model

import com.ferb.app.Note
import com.ferb.design.components.SessionLine
import com.ferb.design.components.SessionLineKind
import com.ferb.design.components.SessionState
import com.ferb.mobile.AgentSession
import com.ferb.mobile.SessionCard
import com.ferb.mobile.SessionItem
import com.ferb.mobile.SessionYes

// The owner's coding sessions on his Mac (kb HAND-651..666), as the phone holds them while their screens show: memory only,
// never stored, gone when the app locks.

/** The Sessions tab: whether the owner turned them on, the sessions, or why they couldn't be read. */
data class SessionsUi(
    val loaded: Boolean = false,
    val enabled: Boolean = true,
    val sessions: List<AgentSession> = emptyList(),
    val problem: Note? = null,
)

/**
 * One session's screen: the session as the Mac last said it, its conversation (oldest first, by `seq`), whether earlier
 * items exist ([more]), the agent's open questions, the owner's draft, and what is open over it: the Mac's question
 * before FERB types for him ([ask]) or the hand-over to FERB.
 */
data class SessionUi(
    val id: String,
    val session: AgentSession? = null,
    val items: List<SessionItem> = emptyList(),
    val more: Boolean = false,
    val cards: List<SessionCard> = emptyList(),
    val loaded: Boolean = false,
    val draft: String = "",
    val ask: SessionConfirm? = null,
    val handOver: Boolean = false,
    val problem: Note? = null,
    /** A message the Mac refused (PHN-800): shown as not sent, with why and Try again; never as sent. */
    val unsent: Unsent? = null,
    /** The session's menu is open (PHN-800). */
    val menu: Boolean = false,
    /** HAND-683: FERB can't tell which terminal is the session's: he chooses one for these words, or doesn't send. */
    val pick: TerminalPick? = null,
) {
    /** The newest item's `seq` (0: none yet): the next poll asks for what came after it. */
    val last: Long get() = items.lastOrNull()?.seq?.toLong() ?: 0L

    /** The oldest item's `seq`: "Show earlier" asks for what came before it. */
    val first: Long get() = items.firstOrNull()?.seq?.toLong() ?: 0L
}

/**
 * The Mac's question before FERB types into a session for the owner (GATE-622): [text] exactly as it will be sent (empty
 * when he only takes control, to stop its turn: [thenInterrupt]).
 */
data class Unsent(val text: String, val reason: Note)

/** HAND-683: the words waiting for his choice of terminal, the Mac's reason, and the terminals it can see. */
data class TerminalPick(val text: String, val why: String, val terminals: List<com.ferb.mobile.SessionChoice>)

data class SessionConfirm(val text: String, val yes: SessionYes, val thenInterrupt: Boolean = false)

/** The New session sheet: the agent, the folders the Mac offers, the first words, and the exact command once chosen. */
data class NewSessionUi(
    val loading: Boolean = true,
    val folders: List<String> = emptyList(),
    val agent: String = Sessions.CLAUDE,
    val folder: String? = null,
    val text: String = "",
    /** The Mac's question with the exact command it will type in Terminal: the owner's Yes starts it. */
    val yes: SessionYes? = null,
)

object Sessions {
    const val CLAUDE = "claude_code"
    const val CODEX = "codex"

    /** The two agents FERB can start, by their product names (not words to translate). */
    val AGENTS = listOf(CLAUDE to "Claude Code", CODEX to "Codex")

    /** The newest items a session's screen asks for at once, and per "Show earlier". */
    const val PAGE = 50

    /** At most this many items per poll after the newest one held (the Mac's own limit is 100). */
    const val AFTER_PAGE = 100

    /** The Sessions tab reads the list this often while it shows (kb HAND-651..666: polled only while visible). */
    const val LIST_MS = 3_000L

    /** An open session reads what came after its newest item this often while its screen shows. */
    const val OPEN_MS = 2_000L

    fun state(status: String): SessionState = when (status) {
        "working" -> SessionState.WORKING
        "waiting_for_you" -> SessionState.WAITING
        else -> SessionState.IDLE
    }

    /** A row's title: the agent's own title, else the folder's name. */
    fun title(s: AgentSession): String = s.title.ifBlank { s.folderName.ifBlank { s.folder } }

    /** New items after what is held, by `seq` (a poll can repeat the last one). */
    fun appended(held: List<SessionItem>, more: List<SessionItem>): List<SessionItem> {
        val last = held.lastOrNull()?.seq ?: 0uL
        return held + more.filter { it.seq > last }.sortedBy { it.seq }
    }

    /** An earlier page before what is held. */
    fun prepended(held: List<SessionItem>, earlier: List<SessionItem>): List<SessionItem> {
        val first = held.firstOrNull()?.seq ?: ULong.MAX_VALUE
        return earlier.filter { it.seq < first }.sortedBy { it.seq } + held
    }

    /** One item as the screen draws it; [stopped] is the app's words for a stop the Mac left without its own. */
    fun line(i: SessionItem, stopped: String): SessionLine {
        val kind = when (i.kind) {
            "user" -> SessionLineKind.OWNER
            "tool" -> SessionLineKind.TOOL
            "interrupted" -> SessionLineKind.INTERRUPTED
            "error" -> SessionLineKind.ERROR
            else -> SessionLineKind.AGENT
        }
        val text = when (kind) {
            SessionLineKind.TOOL -> i.text.ifBlank { i.tool }
            SessionLineKind.INTERRUPTED -> i.text.ifBlank { stopped }
            else -> i.text
        }
        return SessionLine(i.seq.toString(), kind, text)
    }
}
