package com.ferb.app

import com.ferb.mobile.TranscriptView

/**
 * Invented conversations as the Mac stores them (ENG-8 entries), read by the real phone library through [FixtureMac]:
 * the cases where FERB.app and the phone used to differ (kb PHN-720..), for the row tests and the goldens.
 */
object Parity {
    private var pos = 10L

    fun entry(id: String, kind: String, request: String, body: Map<String, Any?>, task: String? = null, state: String? = null,
              answers: String? = null, at: String = "2026-01-02T09:00:00+00:00"): Map<String, Any?> {
        pos++
        return mapOf(
            "id" to id, "conversation_id" to "C1", "pos" to pos.toDouble(), "seq" to pos.toDouble(), "rev" to 1.0, "kind" to kind,
            "state" to (state ?: if (kind == "message") "done" else "done"), "open" to (kind == "question"),
            "request_id" to request, "task_id" to task, "approval_id" to (body["approval_id"]), "ref" to null,
            "answers" to answers, "placement" to if (kind == "answer") "inline" else null, "origin" to null, "client_id" to "phone-1",
            "submission_id" to null, "journal_id" to null, "created_at" to at, "updated_at" to at, "day" to "2026-01-02",
            "body" to body, "deleted" to false,
        )
    }

    fun message(id: String, request: String, text: String, task: String? = "t-1") =
        entry(id, "message", request, mapOf("text" to text, "attachments" to emptyList<String>(), "hidden" to emptyList<String>()), task = task)

    fun answer(id: String, request: String, of: String, text: String, lines: List<String> = emptyList(), cards: List<Map<String, Any?>> = emptyList(),
               artifacts: List<String> = emptyList(), note: String? = null, task: String? = "t-1", status: String = "done") =
        entry(id, "answer", request, mapOf("from" to "task", "text" to text, "status" to status, "verified" to true, "artifacts" to artifacts,
            "note" to note, "lines" to lines, "cards" to cards), task = task, answers = of)

    fun view(vararg entries: Map<String, Any?>): TranscriptView =
        FixtureMac.transcript(FixtureMac(FixtureConversation("invented", "parity", "C1", entries.toList())))

    /** The owner's research answer of 2026-11-18 02:18 (chat entry 95f9ee54): six link cards, the sites line twice. */
    const val SITES_LINE = "Sites contacted: 6 sites"
    val HOSTS = listOf("developer.chrome.com", "developer.mozilla.org", "developers.cloudflare.com", "w3c.github.io", "www.google.com", "www.linkedin.com")
    val RESEARCH = "**Yes, a website can sometimes infer automated interaction, but it usually cannot directly see “an accessibility API " +
        "or extension clicked this.”** Detectability depends mainly on how the action is generated.\n\n" +
        "- **Accessibility/native-browser actions:** the site generally receives ordinary focus, keyboard and pointer events.\n" +
        "- **DOM or synthetic events:** a page can sometimes tell them apart (`isTrusted` is false).\n" +
        "- **Account rules:** LinkedIn prohibits unauthorized automated access. [LinkedIn User Agreement](https://www.linkedin.com/legal/user-agreement)\n\n" +
        (1..14).joinToString("\n\n") { "Paragraph $it of the answer goes on about fingerprints, rate limits and the session as a whole, so that the answer is long." } +
        "\n\n**Bottom line:** a site usually cannot conclusively identify extension-driven accessibility interaction from one normal action.\n\n$SITES_LINE"

    fun research(): TranscriptView = view(
        message("m1", "r1", "search online and then tell"),
        entry("p1", "plan", "r1", mapOf("text" to "I’ll research current technical and policy sources online, then give you a sourced answer.",
            "plan_line" to "I’ll research current technical and policy sources online, then give you a sourced answer.", "lines" to emptyList<String>(),
            "name" to "Research automation detection"), task = "t-1"),
        answer("a1", "r1", "m1", RESEARCH, lines = listOf(SITES_LINE), cards = HOSTS.map { mapOf("type" to "link", "host" to it, "line" to SITES_LINE) }),
    )
}
