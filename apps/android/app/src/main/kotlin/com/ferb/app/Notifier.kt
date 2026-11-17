package com.ferb.app

import android.Manifest
import android.app.KeyguardManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import com.ferb.app.model.ChatModel
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.Questions
import com.ferb.app.model.WorkModel
import com.ferb.core.Outcome
import com.ferb.design.components.MarkKind
import com.ferb.mobile.Change
import com.ferb.mobile.Item
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.TranscriptView
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * FERB's notifications (PHN-415, PHN-417, PHN-421, PHN-422, PHN-125, PHN-126): a question, a task that finished or
 * couldn't, and a dropped link the owner must act on. They come from two places, each read when the core announces a
 * change (no polling): the Mac's status (`Change.Status`: what waits, each question's needs, what finished; it keeps
 * coming while the app is locked) and, while unlocked, the transcript (the question's full words, its exact thing).
 *
 * What a question's notification carries follows the Mac's needs for it, never the app's judgement:
 * - routine (the Mac takes the answer with no proof): its words and up to three answer buttons, or a typed answer;
 * - needs the phone's proof: its words and no buttons, "Needs your fingerprint. Tap to answer it in FERB.";
 * - only on the Mac (can_answer false): its words and that it is answered on the Mac;
 * - a hidden input: never worded (PHN-422), "Open FERB to answer.";
 * - words held back (a locked session that didn't prove the device key, PHN-126): "FERB has a question", wordless;
 * - one of FERB's own checks while the phone is locked and the owner's setting is on: wordless, no buttons (PHN-422).
 * Unknown needs (no status yet) are treated as needing the app: no buttons. Every notification is VISIBILITY_PRIVATE
 * with a public version; no picture is ever put in one.
 */
class Notifier(
    private val context: Context,
    private val api: PhoneApi,
    private val prefs: Prefs,
    private val session: Session,
) {
    private val nm = context.getSystemService(NotificationManager::class.java)
    private val keyguard = context.getSystemService(KeyguardManager::class.java)
    private val gate = Mutex()

    /** Questions on show now (re-posted when the phone locks or unlocks: FERB's checks change their words then). */
    private val posted = ConcurrentHashMap<String, Question>()
    private var primedTranscript = false
    private var primedStatus = false

    fun start(scope: CoroutineScope) {
        channels()
        // Android's own lock-screen setting decides what a private notification shows there; a channel can't force it
        // (the system ignores an app's lockscreenVisibility: measured on the emulator, S11). So a check is re-posted
        // without its words when the screen goes off, and with them when the owner unlocks the phone.
        val screen = object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                val locked = i.action == Intent.ACTION_SCREEN_OFF
                for (q in posted.values) if (q.ownCheck) question(q, lockedNow = locked, quietly = true)
            }
        }
        context.registerReceiver(screen, IntentFilter().apply { addAction(Intent.ACTION_SCREEN_OFF); addAction(Intent.ACTION_USER_PRESENT) },
            Context.RECEIVER_NOT_EXPORTED)
        scope.launch {
            api.changes.collect { c ->
                when (c) {
                    is Change.Transcript, is Change.Live -> scope.launch { lookTranscript() }
                    is Change.Status -> scope.launch { lookStatus() }
                    // the Mac asks for the exact location (GATE-613): with the app away, a notification; the sheet
                    // that answers is in the app only (no button here shares anything), kb PHN-537
                    is Change.LocationWanted -> if (!quiet) location(c.requestId)
                    else -> Unit
                }
            }
        }
        scope.launch {
            var last: LinkPhase? = null
            api.link.collect { l ->
                val code = l?.problem?.code
                if (l?.phase == LinkPhase.STOPPED && last != LinkPhase.STOPPED && code in FerbViewModel.REMOVED_CODES) {
                    post(KEY_LINK, CH_LINK, context.getString(R.string.n_link_removed_title), context.getString(R.string.n_link_removed_text), open(null))
                }
                last = l?.phase
            }
        }
    }

    /** Should a notification show now: not while the owner is looking at the open app. */
    private val quiet: Boolean get() = session.front.value && !session.locked.value

    /** The Mac's status: questions with their needs, and what finished (held: wordless). Works while locked. */
    private suspend fun lookStatus() = gate.withLock {
        val status = api.lastStatus() ?: return@withLock
        val announced = prefs.announced.toMutableSet()
        val view = if (session.locked.value) null else (api.transcript() as? Outcome.Ok)?.value
        val questions = Questions.merged(view?.let { Questions.open(it.questions) }.orEmpty().filter { q -> status.questions.any { it.approvalId == q.approvalId } }, status)
        if (!primedStatus) {
            primedStatus = true
            announced += questions.map { "q:${it.approvalId}" } + status.results.map { "t:${it.taskId}" }
            prefs.announced = announced
            return@withLock
        }
        for (q in questions) {
            val key = "q:${q.approvalId}"
            if (key in announced) {
                // known already: its words or needs may have changed (the app unlocked, a status arrived)
                if (posted.containsKey(q.approvalId) && posted[q.approvalId] != q) question(q, quietly = true)
                continue
            }
            announced += key
            if (!quiet) question(q)
        }
        // what the Mac no longer has waiting goes from the shade
        val open = status.questions.map { it.approvalId }.toSet()
        for (id in posted.keys.toList()) if (id !in open) cancel(id)
        for (r in status.results) {
            val key = "t:${r.taskId}"
            if (key in announced) continue
            announced += key
            if (quiet) continue
            val failed = ChatModel.markOf(r.status) == MarkKind.FAILED
            if (r.held) {
                post(key, CH_DONE, context.getString(R.string.n_done_held), context.getString(R.string.n_held_text), open(null))
            } else {
                val title = context.getString(if (failed) R.string.n_task_failed else R.string.n_task_done)
                val line = WorkModel.oneLine(r.name.ifEmpty { ChatModel.plain(r.answer) })
                post(key, CH_DONE, title, line, open(null))
            }
        }
        prefs.announced = announced
    }

    /** The transcript (unlocked): the questions' full words, with their needs from the last status. */
    private suspend fun lookTranscript() = gate.withLock {
        val view = (api.transcript() as? Outcome.Ok)?.value ?: return@withLock
        val announced = prefs.announced.toMutableSet()
        val status = api.lastStatus()
        val open = Questions.open(view.questions).let { Questions.merged(it, status) }.filter { q -> view.questions.any { it.approvalId == q.approvalId } }
        val terminal = finished(view)
        if (!primedTranscript) {
            // what was already there when the app started is not news
            primedTranscript = true
            announced += open.map { "q:${it.approvalId}" } + terminal.map { "t:${it.first}" }
            prefs.announced = announced
            return@withLock
        }
        for (q in open) {
            val key = "q:${q.approvalId}"
            if (key in announced) {
                if (posted.containsKey(q.approvalId) && posted[q.approvalId] != q) question(q, quietly = true)
                continue
            }
            announced += key
            if (!quiet) question(q)
        }
        // a question answered elsewhere, or no longer waiting: its notification goes
        val still = open.map { it.approvalId }.toSet()
        for (id in posted.keys.toList()) if (id !in still) cancel(id)
        for ((task, words) in terminal) {
            val key = "t:$task"
            if (key in announced) continue
            announced += key
            if (!quiet) post(key, CH_DONE, words.first, words.second, open(task))
        }
        prefs.announced = announced
    }

    private fun cancel(approvalId: String) {
        nm.cancel(keyOf("q:$approvalId"))
        posted.remove(approvalId)
    }

    /** The turns that finished, failed or were stopped, by task id, with their title and first line. */
    private fun finished(view: TranscriptView): List<Pair<String, Pair<String, String>>> =
        view.items.mapNotNull { (it as? Item.Turn)?.turn }
            .filter { it.taskId != null && it.answer != null && it.status !in setOf("working", "needs_user", "merged") }
            .map { t ->
                val failed = ChatModel.markOf(t.status) == MarkKind.FAILED
                val title = context.getString(if (failed) R.string.n_task_failed else R.string.n_task_done)
                val line = WorkModel.title(t).ifEmpty { WorkModel.oneLine(ChatModel.plain(t.answer!!.text)) }
                t.taskId!! to (title to line)
            }

    /** A question's notification, worded and with buttons as its needs allow (the class comment). */
    fun question(q: Question, lockedNow: Boolean = keyguard.isKeyguardLocked, quietly: Boolean = false) {
        val id = q.approvalId
        val wordless = q.ownCheck && prefs.hideChecksOnLockScreen && lockedNow
        val asking = q.title ?: context.getString(R.string.n_question_title)
        val (title, text) = when {
            q.hidden -> context.getString(R.string.n_private_title) to context.getString(R.string.n_private_text)
            q.held -> context.getString(R.string.n_question_title) to context.getString(R.string.n_held_text)
            wordless -> context.getString(R.string.n_check_title) to context.getString(R.string.n_check_text)
            q.onlyOnMac -> asking to listOfNotNull(q.reason.ifEmpty { null }, context.getString(R.string.n_on_mac)).joinToString("\n")
            !q.routine -> asking to listOfNotNull(q.reason.ifEmpty { null }, context.getString(when {
                q.needs == null -> R.string.n_needs_fingerprint_text
                q.needs.fingerprint -> R.string.n_needs_fingerprint
                else -> R.string.n_needs_unlock
            })).joinToString("\n")
            else -> asking to q.reason.ifEmpty { asking }
        }
        val b = builder(if (q.ownCheck) CH_CHECKS else CH_QUESTIONS, title, text, open(null, id)).setOnlyAlertOnce(quietly)
        if (q.routine && !wordless && !q.held) {
            val choices = q.choices
            if (choices.isNotEmpty() && choices.size <= 3) {
                // Yes first where there is one, as the phone's sheet reads left to right with Yes on the right
                choices.sortedByDescending { it.yes }.forEach { c ->
                    b.addAction(Notification.Action.Builder(null, c.label, answerIntent(id, q.choices.indexOf(c), null)).build())
                }
                // Android shows three actions: "until I'm back at my Mac" when the Mac offers it and there is room (PHN-611)
                if (q.untilBack && choices.size < 3) {
                    b.addAction(Notification.Action.Builder(null, context.getString(R.string.until_back_yes),
                        answerIntent(id, AnswerReceiver.UNTIL_BACK, null)).build())
                }
            } else if (q.options.isEmpty()) {
                val input = RemoteInput.Builder(AnswerReceiver.TEXT).setLabel(context.getString(R.string.n_answer_hint)).build()
                b.addAction(Notification.Action.Builder(null, context.getString(R.string.n_answer), answerIntent(id, -1, true)).addRemoteInput(input).build())
            }
        }
        posted[id] = q
        notify(keyOf("q:$id"), b.build())
    }

    /** The Mac's request for this phone's exact location: words and a tap that opens the app's sheet; no buttons. */
    fun location(requestId: String) {
        val i = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra(MainActivity.EXTRA_LOCATION, requestId)
        val tap = PendingIntent.getActivity(context, "loc:$requestId".hashCode(), i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        post(KEY_LOCATION, CH_QUESTIONS, context.getString(R.string.n_exact_title), context.getString(R.string.n_exact_text), tap)
    }

    /** The request was answered in the app: its notification goes. */
    fun locationAnswered() = nm.cancel(keyOf(KEY_LOCATION))

    /** After an answer from the notification: gone, or replaced by what stopped it (PHN-302). */
    fun answered(approvalId: String, problem: String?, needsApp: Boolean) {
        val key = keyOf("q:$approvalId")
        if (problem == null) { nm.cancel(key); posted.remove(approvalId); return }
        val title = context.getString(if (needsApp) R.string.n_needs_fingerprint_title else R.string.n_not_answered_title)
        notify(key, builder(CH_QUESTIONS, title, problem, open(null, approvalId)).build())
    }

    private fun post(key: String, channel: String, title: String, text: String, tap: PendingIntent) =
        notify(keyOf(key), builder(channel, title, text, tap).build())

    private fun builder(channel: String, title: String, text: String, tap: PendingIntent): Notification.Builder {
        val public = Notification.Builder(context, channel).setSmallIcon(R.drawable.ic_stat_ferb)
            .setContentTitle(context.getString(R.string.app_name))
            .setContentText(context.getString(if (channel == CH_DONE) R.string.n_public_done else R.string.n_public_question))
            .build()
        return Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_ferb)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public)
            .setCategory(if (channel == CH_DONE) Notification.CATEGORY_STATUS else Notification.CATEGORY_MESSAGE)
            .setAutoCancel(true)
            .setContentIntent(tap)
    }

    private fun notify(id: Int, n: Notification) {
        if (context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        nm.notify(id, n)
    }

    private fun open(entry: String?, approvalId: String? = null): PendingIntent {
        val i = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        approvalId?.let { i.putExtra(MainActivity.EXTRA_QUESTION, it) }
        entry?.let { i.putExtra(MainActivity.EXTRA_ENTRY, it) }
        return PendingIntent.getActivity(context, (approvalId ?: entry ?: "open").hashCode(), i,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun answerIntent(approvalId: String, choice: Int, text: Boolean?): PendingIntent {
        val i = Intent(context, AnswerReceiver::class.java).setAction(AnswerReceiver.ACTION)
            .putExtra(AnswerReceiver.ID, approvalId).putExtra(AnswerReceiver.CHOICE, choice)
        // a typed answer needs a mutable intent (the system adds the words); a button's is immutable
        val flags = if (text == true) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getBroadcast(context, "$approvalId:$choice".hashCode(), i, flags or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    /** The channels: questions, FERB's checks (never worded on the lock screen, PHN-422), finished tasks, the link. */
    fun channels() {
        fun ch(id: String, name: Int, desc: Int, importance: Int) =
            NotificationChannel(id, context.getString(name), importance).apply { description = context.getString(desc) }
        nm.createNotificationChannels(listOf(
            ch(CH_QUESTIONS, R.string.ch_questions, R.string.ch_questions_desc, NotificationManager.IMPORTANCE_HIGH),
            ch(CH_CHECKS, R.string.ch_checks, R.string.ch_checks_desc, NotificationManager.IMPORTANCE_HIGH),
            ch(CH_DONE, R.string.ch_done, R.string.ch_done_desc, NotificationManager.IMPORTANCE_DEFAULT),
            ch(CH_LINK, R.string.ch_link, R.string.ch_link_desc, NotificationManager.IMPORTANCE_HIGH),
            ch(CH_STAY, R.string.ch_stay, R.string.ch_stay_desc, NotificationManager.IMPORTANCE_MIN),
        ))
    }

    fun clearAll() = nm.cancelAll()

    companion object {
        const val CH_QUESTIONS = "questions"
        const val CH_CHECKS = "checks"
        const val CH_DONE = "finished"
        const val CH_LINK = "connection"
        const val CH_STAY = "stay_connected"
        const val KEY_LINK = "link"
        const val KEY_LOCATION = "location"
        fun keyOf(key: String): Int = key.hashCode()

        /** The questions a status brings, for a test or a receiver: each with its needs. */
        fun questionsOf(status: PhoneStatus?): List<Question> = Questions.merged(emptyList(), status)
    }
}
