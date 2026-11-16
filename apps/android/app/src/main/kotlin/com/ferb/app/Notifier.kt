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
import android.content.IntentFilter
import android.content.Intent
import android.content.pm.PackageManager
import com.ferb.app.model.ChatModel
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.Questions
import com.ferb.app.model.WorkModel
import com.ferb.core.Outcome
import com.ferb.mobile.Change
import com.ferb.mobile.Item
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.TranscriptView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * FERB's notifications (PHN-415, PHN-417, PHN-421, PHN-422): a question (answerable from its buttons), a task that
 * finished or couldn't, and a dropped link the owner must act on. They are posted from what the link brings while
 * the process runs (the app in front, or Stay connected's service); with the app swiped away and Stay connected off,
 * nothing arrives (closed-app push is stage 5). Nothing private beyond the decisions: on the lock screen the text
 * follows the phone's own setting (VISIBILITY_PRIVATE with a public version), FERB's own checks show no words there
 * at all while the owner's setting says so, a private answer's question is never worded, and no picture is ever put
 * in one. No polling: each read follows a change the core announced.
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

    /** FERB's checks on show now: worded again when the phone is unlocked, wordless when it locks (PHN-422). */
    private val checks = java.util.concurrent.ConcurrentHashMap<String, Question>()

    /** Ids posted this process (questions open now); the persisted set says what was announced before. */
    private val shown = HashSet<String>()
    private var primed = false

    fun start(scope: CoroutineScope) {
        channels()
        // Android's own lock-screen setting decides what a private notification shows there; a channel can't force it
        // (the system ignores an app's lockscreenVisibility: measured on the emulator, S11). So a check is re-posted
        // without its words when the screen goes off, and with them when the owner unlocks the phone.
        val screen = object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                val locked = i.action == Intent.ACTION_SCREEN_OFF
                for (q in checks.values) question(q, lockedNow = locked, quietly = true)
            }
        }
        context.registerReceiver(screen, IntentFilter().apply { addAction(Intent.ACTION_SCREEN_OFF); addAction(Intent.ACTION_USER_PRESENT) },
            Context.RECEIVER_NOT_EXPORTED)
        scope.launch { api.changes.collect { c -> if (c is Change.Transcript || c is Change.Live) scope.launch { look() } } }
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

    private suspend fun look() = gate.withLock {
        val view = (api.transcript() as? Outcome.Ok)?.value ?: return@withLock
        val announced = prefs.announced.toMutableSet()
        val open = Questions.open(view.questions)
        val terminal = finished(view)
        if (!primed) {
            // what was already there when the app started is not news
            primed = true
            announced += open.map { "q:${it.approvalId}" } + terminal.map { "t:${it.first}" }
            prefs.announced = announced
            return@withLock
        }
        for (q in open) {
            val key = "q:${q.approvalId}"
            if (key in announced) continue
            announced += key
            if (!quiet) question(q)
        }
        // a question answered elsewhere, or no longer waiting: its notification goes
        val still = open.map { it.approvalId }.toSet()
        for (id in shown.toList()) if (id !in still) { nm.cancel(keyOf("q:$id")); shown.remove(id); checks.remove(id) }
        for ((entry, words) in terminal) {
            val key = "t:$entry"
            if (key in announced) continue
            announced += key
            if (!quiet) post(key, CH_DONE, words.first, words.second, open(entry))
        }
        prefs.announced = announced
    }

    /** The turns that finished, failed or were stopped, with their title and first line. */
    private fun finished(view: TranscriptView): List<Pair<String, Pair<String, String>>> =
        view.items.mapNotNull { (it as? Item.Turn)?.turn }
            .filter { it.taskId != null && it.answer != null && it.status !in setOf("working", "needs_user", "merged") }
            .map { t ->
                val failed = ChatModel.markOf(t.status) == com.ferb.design.components.MarkKind.FAILED
                val title = context.getString(if (failed) R.string.n_task_failed else R.string.n_task_done)
                val line = WorkModel.title(t).ifEmpty { WorkModel.oneLine(ChatModel.plain(t.answer!!.text)) }
                t.entry to (title to line)
            }

    /**
     * A question: its words (not a private answer's), and its answers as buttons when it has at most three. One of
     * FERB's own checks, while the owner keeps PHN-422's setting on, carries no words and no buttons while the phone
     * is locked: the lock screen never shows what it is about.
     */
    fun question(q: Question, lockedNow: Boolean = keyguard.isKeyguardLocked, quietly: Boolean = false) {
        val id = q.approvalId
        val wordless = q.ownCheck && prefs.hideChecksOnLockScreen && lockedNow
        val title = when {
            q.hidden -> context.getString(R.string.n_private_title)
            wordless -> context.getString(R.string.n_check_title)
            else -> q.title ?: context.getString(R.string.n_question_title)
        }
        val text = when {
            q.hidden -> context.getString(R.string.n_private_text)
            wordless -> context.getString(R.string.n_check_text)
            else -> q.reason.ifEmpty { title }
        }
        val b = builder(if (q.ownCheck) CH_CHECKS else CH_QUESTIONS, title, text, open(null, id)).setOnlyAlertOnce(quietly)
        if (q.ownCheck) checks[id] = q
        if (!q.hidden && !wordless) {
            val choices = q.choices
            if (choices.isNotEmpty() && choices.size <= 3) {
                // Yes first where there is one, as the phone's sheet reads left to right with Yes on the right
                choices.sortedByDescending { it.yes }.forEach { c ->
                    b.addAction(Notification.Action.Builder(null, c.label, answerIntent(id, q.choices.indexOf(c), null)).build())
                }
            } else if (q.options.isEmpty()) {
                val input = RemoteInput.Builder(AnswerReceiver.TEXT).setLabel(context.getString(R.string.n_answer_hint)).build()
                b.addAction(Notification.Action.Builder(null, context.getString(R.string.n_answer), answerIntent(id, -1, true)).addRemoteInput(input).build())
            }
        }
        shown.add(id)
        notify(keyOf("q:$id"), b.build())
    }

    /** After an answer from the notification: gone, or replaced by what stopped it (PHN-302). */
    fun answered(approvalId: String, problem: String?, needsApp: Boolean) {
        val key = keyOf("q:$approvalId")
        checks.remove(approvalId)
        if (problem == null) { nm.cancel(key); shown.remove(approvalId); return }
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
        fun keyOf(key: String): Int = key.hashCode()
    }
}
