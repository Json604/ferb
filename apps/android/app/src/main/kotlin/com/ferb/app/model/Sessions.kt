package com.ferb.app.model

import com.ferb.app.Note
import com.ferb.design.components.RichBlock
import com.ferb.design.components.SessionLine
import com.ferb.design.components.SessionPartKind
import com.ferb.design.components.SessionPartView
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
    /** HAND-697: the chat apps open on the Mac with no chat FERB could find. */
    val noChats: List<String> = emptyList(),
    /**
     * A new terminal the Mac opened before its session showed (its start answered no session yet): the sessions the list
     * held then; the list says it will show in a moment until a session not among them arrives.
     */
    val coming: Set<String>? = null,
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
    /**
     * His message on its way (PHN-812: Sending…, then as sent until the session's own record of it arrives) or refused
     * (PHN-800: not sent, with why and Try again; never as sent). One bubble after the conversation's last line.
     */
    val outgoing: SessionSending? = null,
    /** The session's menu is open (PHN-800). */
    val menu: Boolean = false,
    /** HAND-683: FERB can't tell which terminal is the session's: he chooses one for these words, or doesn't send. */
    val pick: TerminalPick? = null,
    /** The Mac refused the hand-over: its reason under Hand it over; the sheet keeps his words (HAND-702). */
    val handProblem: Note? = null,
) {
    /** The newest item's `seq` (0: none yet): the next poll asks for what came after it. */
    val last: Long get() = items.lastOrNull()?.seq?.toLong() ?: 0L

    /** The oldest item's `seq`: "Show earlier" asks for what came before it. */
    val first: Long get() = items.firstOrNull()?.seq?.toLong() ?: 0L
}

/** Where his message to a session stands (PHN-812, PHN-800). */
enum class SessionSendPhase {
    /** FERB is typing it into the session's terminal on the Mac: "Sending…" with a turning mark. */
    SENDING,

    /** The Mac typed it: a plain bubble of his until the session's own record of a message of his arrives. */
    SENT,

    /** The Mac refused it or it never reached the Mac: dimmed, "Not sent." with why, and Try again. */
    NOT_SENT,
}

/**
 * His message to a session that the conversation doesn't hold yet (one state for sending, sent and not sent: PHN-812,
 * PHN-800): [text] as he wrote it, [phase], the Mac's [reason] when not sent, and [after], the conversation's newest
 * `seq` when he sent it (a message of his after it is the session's own record of this one).
 */
data class SessionSending(val text: String, val phase: SessionSendPhase, val reason: Note? = null, val after: Long = 0)

/** HAND-683: the words waiting for his choice of terminal, the Mac's reason, and the terminals it can see. */
data class TerminalPick(val text: String, val why: String, val terminals: List<com.ferb.mobile.SessionChoice>)

/**
 * The Mac's question before FERB types into a session for the owner (GATE-622): [text] exactly as it will be sent (empty
 * when he only takes control, to stop its turn: [thenInterrupt]).
 */
data class SessionConfirm(val text: String, val yes: SessionYes, val thenInterrupt: Boolean = false)

/**
 * The New session sheet (HAND-691): a terminal, so only where (the folders the Mac offers, or another chosen in the Mac's
 * folders: [picked], PHN-821) and an optional command ([text]); with a command, the Mac's question showing exactly it.
 */
data class NewSessionUi(
    val loading: Boolean = true,
    val folders: List<String> = emptyList(),
    val folder: String? = null,
    val text: String = "",
    /** The Mac's question with the exact command it will type in the new terminal: the owner's Yes starts it. */
    val yes: SessionYes? = null,
    /** A folder he chose in the Mac's folders ("Another folder…"): first in the list, above the Mac's own. */
    val picked: String? = null,
    /** The Mac's folders open over the sheet, to choose another folder (folders only). */
    val browse: FolderPickUi? = null,
    /** The Mac refused to open it: its reason under Start; the sheet keeps what he chose. */
    val problem: Note? = null,
) {
    /** The folder it opens in. */
    val chosen: String? get() = folder

    /** The folders the sheet lists: the one he chose in the Mac's folders first, then the Mac's own as it sent them. */
    val rows: List<String> get() = listOfNotNull(picked) + folders.filter { it != picked }
}

/**
 * Choosing another folder in the Mac's folders (PHN-821): the Files tab's browser over `phone.files` in a folders-only
 * mode. [trail] the folders opened from the places, the last one showing (empty: the places and the recent folders);
 * [loading] the path on its way; [problem] why the last one couldn't open.
 */
data class FolderPickUi(
    val trail: List<com.ferb.mobile.Folder> = emptyList(),
    val loading: String? = null,
    val problem: Note? = null,
    val recent: List<String> = emptyList(),
) {
    val shown: com.ferb.mobile.Folder? get() = trail.lastOrNull()
}

object Sessions {
    /** HAND-691: what a new session is (the only kind FERB opens). */
    const val TERMINAL = "terminal"

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

    /**
     * A row's title, never empty: the agent's own title, else its window's in its app's words, else the folder's name,
     * else the agent's name ("Claude Code", "Codex", the app's, the terminal program, "Terminal").
     */
    fun title(s: AgentSession): String = s.title.ifBlank { s.window }.ifBlank { s.folderName }
        .ifBlank { s.folder.trimEnd('/').substringAfterLast('/') }.ifBlank { s.folder }.ifBlank { s.agentName }.ifBlank { s.agent }

    /** Whether the session is a terminal's (its output is shown as it is: never Markdown). */
    fun terminal(s: AgentSession?): Boolean = s?.agent == TERMINAL

    /**
     * Reads the timeline's Markdown through the phone library and keeps it (Present.rich), off the main thread, so the
     * screen's lines find every one kept (as ChatModel.prepare for the chat).
     */
    fun prepare(t: com.ferb.mobile.SessionTimeline) {
        val terminal = terminal(t.session)
        for (i in t.items) markdownOf(i, terminal).forEach { Present.rich(it) }
    }

    /** The Markdown of one item (HAND-703): its text parts, else an agent's words (never a terminal's output). */
    private fun markdownOf(i: SessionItem, terminal: Boolean): List<String> = when {
        i.parts.isNotEmpty() -> i.parts.filter { partKind(it.kind) == SessionPartKind.TEXT && it.text.isNotBlank() }.map { it.text }
        lineKind(i.kind) == SessionLineKind.AGENT && !terminal && i.text.isNotBlank() -> listOf(i.text)
        else -> emptyList()
    }

    /** A part's kind; one the app doesn't know is drawn as words. */
    fun partKind(kind: String): SessionPartKind = when (kind) {
        "thinking" -> SessionPartKind.THINKING
        "tool" -> SessionPartKind.TOOL
        "meta" -> SessionPartKind.META
        else -> SessionPartKind.TEXT
    }

    private fun lineKind(kind: String) = when (kind) {
        "user" -> SessionLineKind.OWNER
        "tool" -> SessionLineKind.TOOL
        "interrupted" -> SessionLineKind.INTERRUPTED
        "error" -> SessionLineKind.ERROR
        else -> SessionLineKind.AGENT
    }

    /** New items after what is held, by `seq` (a poll can repeat the last one). */
    fun appended(held: List<SessionItem>, more: List<SessionItem>): List<SessionItem> {
        val last = held.lastOrNull()?.seq ?: 0uL
        return held + more.filter { it.seq > last }.sortedBy { it.seq }
    }

    /**
     * A sent message's bubble leaves once the conversation holds a message of his after the moment he sent it (the
     * session's own record of it: the agent may write it a little differently, so any message of his counts); sending
     * and not sent stay until the Mac answers or he tries again (PHN-812).
     */
    fun settled(o: SessionSending?, items: List<SessionItem>): SessionSending? =
        o?.takeUnless { it.phase == SessionSendPhase.SENT && items.any { i -> i.kind == "user" && i.seq.toLong() > it.after } }

    /** An earlier page before what is held. */
    fun prepended(held: List<SessionItem>, earlier: List<SessionItem>): List<SessionItem> {
        val first = held.firstOrNull()?.seq ?: ULong.MAX_VALUE
        return earlier.filter { it.seq < first }.sortedBy { it.seq } + held
    }

    /**
     * The first line of each day by its `seq`, with that day (the main chat's day, ChatModel.chatDay, in [zone]): the
     * session's lines carry a day's start as the main chat does (PHN-813). An item whose time can't be read starts none.
     */
    fun days(items: List<SessionItem>, zone: java.time.ZoneId): Map<ULong, Long> {
        val out = LinkedHashMap<ULong, Long>()
        var prev: Long? = null
        for (i in items) {
            val ms = runCatching { java.time.Instant.parse(i.at).toEpochMilli() }.getOrNull() ?: continue
            val d = ChatModel.chatDay(ms, zone)
            if (d != prev) out[i.seq] = d
            prev = d
        }
        return out
    }

    /**
     * One item as the screen draws it; [stopped] is the app's words for a stop the Mac left without its own. An agent's
     * reply (HAND-703): its parts when the Mac sent them, else its words as Markdown ([rich], the phone library's
     * reading, kept by [prepare]), except a terminal's output ([terminal]), shown as it is. The owner's words stay plain.
     */
    fun line(
        i: SessionItem, stopped: String, terminal: Boolean = false, rich: (String) -> List<RichBlock> = Present::rich,
        sentByFerb: String = "",
    ): SessionLine {
        val kind = lineKind(i.kind)
        val text = when (kind) {
            SessionLineKind.TOOL -> i.text.ifBlank { i.tool }
            SessionLineKind.INTERRUPTED -> i.text.ifBlank { stopped }
            else -> i.text
        }
        // HAND-709: a message FERB sent while it kept the session going is marked as FERB's
        if (kind == SessionLineKind.OWNER && i.ferb && sentByFerb.isNotEmpty()) {
            return SessionLine(i.seq.toString(), kind, text, by = sentByFerb)
        }
        if (kind != SessionLineKind.AGENT) return SessionLine(i.seq.toString(), kind, text)
        if (i.parts.isNotEmpty()) {
            val parts = i.parts.map { p ->
                when (val k = partKind(p.kind)) {
                    SessionPartKind.TEXT -> SessionPartView(k, blocks = if (p.text.isBlank()) emptyList() else rich(p.text), text = p.text)
                    // the meta line's words are its text (or its label when the Mac put them there)
                    SessionPartKind.META -> SessionPartView(k, text = p.text.ifBlank { p.label })
                    else -> SessionPartView(k, text = p.text, label = p.label.ifBlank { p.text.lineSequence().firstOrNull().orEmpty() }, detail = p.detail)
                }
            }
            return SessionLine(i.seq.toString(), kind, text, parts = parts)
        }
        if (terminal || text.isBlank()) return SessionLine(i.seq.toString(), kind, text)
        return SessionLine(i.seq.toString(), kind, text, blocks = rich(text))
    }
}
