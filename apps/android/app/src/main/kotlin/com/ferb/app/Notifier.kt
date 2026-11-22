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
import com.ferb.app.model.PhoneApi
import com.ferb.app.model.Question
import com.ferb.app.model.QuestionAlerts
import com.ferb.app.model.Questions
import com.ferb.core.Outcome
import com.ferb.mobile.Change
import com.ferb.mobile.Insomnia
import com.ferb.mobile.Item
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.NotificationWords
import com.ferb.mobile.PhoneStatus
import com.ferb.mobile.TranscriptView
import com.ferb.mobile.questionWords
import com.ferb.mobile.taskEndWords
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
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
 * - the planner's question (kb PHN-762): its words and "Tap to answer in FERB.", no buttons; the tap opens the chat;
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
    /** How the can't-reach time is waited (tests pass one that returns at once). */
    private val wait: suspend (Long) -> Unit = { kotlinx.coroutines.delay(it) },
) {
    private val nm = context.getSystemService(NotificationManager::class.java)
    private val keyguard = context.getSystemService(KeyguardManager::class.java)
    private val gate = Mutex()

    /** Questions on show now (re-posted when the phone locks or unlocks: FERB's checks change their words then). */
    private val posted = ConcurrentHashMap<String, Question>()
    /** What had finished when this process started is not news (questions are: each waiting one is posted once). */
    private var primedStatus = false
    private var primedTranscript = false

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
        // one look at a time, and at most one more waiting behind it: a burst of changes (live frames come several
        // times a second) never piles up reads of the library (kb PHN-682)
        val looks = Coalesced(scope) { look() }
        // he leaves the app, comes back, unlocks it: what waits is looked at again (a question seen in the app goes
        // to the shade when he leaves with it still waiting, kb PHN-764)
        scope.launch { session.front.collect { looks.ask() } }
        scope.launch { session.locked.collect { looks.ask() } }
        scope.launch {
            api.changes.collect { c ->
                when (c) {
                    is Change.Transcript, is Change.Live, is Change.Status -> looks.ask()
                    // the Mac asks for the exact location (GATE-613): with the app away, a notification; the sheet
                    // that answers is in the app only (no button here shares anything), kb PHN-537
                    is Change.LocationWanted -> if (!quiet) location(c.requestId)
                    else -> Unit
                }
            }
        }
        scope.launch {
            // the Mac may be asleep (APP-981): the link lost for the can't-reach time while insomnia mode was on; one
            // quiet notification, gone when the link is back
            api.link.map { it?.phase }.distinctUntilChanged().collectLatest { phase ->
                if (phase == null || phase == LinkPhase.ONLINE || phase == LinkPhase.STOPPED) return@collectLatest asleep(false)
                wait(FerbViewModel.LINK_LOST_AFTER_MS)
                asleep(com.ferb.app.model.MacAsleep.shown(api.lastStatus(), api.link.value, lost = true))
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

    /** Should a notification show now: not while the owner has the app on screen (kb PHN-760). */
    private val quiet: Boolean get() = session.front.value

    /** The one rule for questions (kb PHN-760..764): which to post, update and remove. */
    private val alerts = QuestionAlerts()

    /**
     * One look at what waits (kb PHN-760): the Mac's status (every question, the planner's too, with its needs; it keeps
     * coming while the app is locked) and, while unlocked, the transcript (the questions' full words, finished turns).
     * Every question that waits is posted once unless he is in the app ([QuestionAlerts]); what finished is posted
     * once, and what was already finished when this process started is not news.
     */
    private suspend fun look() = gate.withLock {
        val status = api.lastStatus()
        val view = if (session.locked.value) null else (api.transcript() as? Outcome.Ok)?.value
        val inTranscript = view?.let { Questions.open(it.questions) }
        val waiting = when {
            status != null -> Questions.waiting(inTranscript.orEmpty().filter { q -> status.questions.any { it.approvalId == q.approvalId } }, status)
            else -> inTranscript
        }
        val plan = alerts.decide(waiting, session.front.value, !session.locked.value, HashMap(posted), prefs.announced)
        val announced = plan.announced.toMutableSet()
        for (p in plan.post) question(p.question, quietly = p.quiet)
        for (q in plan.update) question(q, quietly = true)
        for (id in plan.cancel) cancel(id)
        // insomnia mode's notice, warnings and how it ended (APP-981, APP-983), from the same status
        if (status != null) insomnia(status.insomnia, announced)
        // what finished: from the status (held words while locked) and the transcript, in the Mac's words (kb PHN-795)
        val terminal = view?.let(::finished).orEmpty()
        // the first status and the first transcript this process reads: what had finished is not news
        if (!primedStatus && status != null) { primedStatus = true; announced += status.results.map { "t:${it.taskId}" } }
        if (!primedTranscript && view != null) { primedTranscript = true; announced += terminal.map { "t:${it.first}" } }
        for (r in status?.results.orEmpty()) {
            val key = "t:${r.taskId}"
            if (key in announced) continue
            // the Mac's banner for it: the task's short name and the answer (none: cancelled, no banner on the Mac)
            val words = taskEndWords(r.status, r.name, r.answer, null, r.held) ?: continue
            announced += key
            if (quiet) continue
            // the words held back (PHN-126): the Mac's title, and only that it can be seen in FERB
            val text = if (r.held) context.getString(R.string.n_held_text) else words.text
            post(key, CH_DONE, words.title, text, open(null))
        }
        for ((task, words) in terminal) {
            val key = "t:$task"
            if (key in announced) continue
            announced += key
            if (!quiet) post(key, CH_DONE, words.title, words.text, open(task))
        }
        prefs.announced = announced
    }

    /**
     * Insomnia mode's notifications (APP-981, APP-983), each from the Mac's status, kept under [TAG_INSOMNIA]:
     * - the notice (someone touched the Mac, or an app opened that FERB didn't): posted once, with Show me (opens the
     *   app's camera view, which needs the app's unlock), Lock my Mac and It's fine (both answered from here, allowed
     *   while the phone is locked); a later touch in the same episode updates it quietly. Posted even while the app is
     *   on screen: someone at his Mac is news wherever he is;
     * - each warning (battery, heat, the cover): posted once, no actions, a tap opens the app;
     * - each is removed when the status no longer has it (answered, gone, the mode ended);
     * - how the mode ended: said once, quietly (in the app when he is there, see FerbViewModel.sayLastEnd).
     * No camera frame is ever put in a notification. An older Mac (no `insomnia`) removes whatever was posted.
     */
    private fun insomnia(i: Insomnia?, announced: MutableSet<String>) {
        val now = HashMap<String, Int>()
        val n = i?.notice?.takeIf { i.on }
        if (n != null) {
            val key = "in:${n.id}"
            now[key] = keyOf(key)
            val title = context.getString(if (n.kind == "activation") R.string.n_insomnia_activation else R.string.n_insomnia_touched)
            val known = key in announced
            val said = "${n.count}|${n.text}"
            // after a restart what was posted is taken as it is (never posted again)
            if (known && insomniaShown[key] == null) insomniaShown[key] = said
            // a later touch of the same episode (its count), or new words from the Mac: updated quietly
            if (!known || insomniaShown[key] != said) {
                val b = builder(CH_INSOMNIA, title, n.text.ifEmpty { title }, open(null), public = R.string.n_public_mac)
                    .setCategory(Notification.CATEGORY_ALARM)
                    .setAutoCancel(false)
                    .setOnlyAlertOnce(known)
                if (known) b.setGroup(SILENT_GROUP).setGroupAlertBehavior(Notification.GROUP_ALERT_SUMMARY)
                b.addAction(Notification.Action.Builder(null, context.getString(R.string.insomnia_show_me), camera()).build())
                b.addAction(Notification.Action.Builder(null, context.getString(R.string.insomnia_lock_mac),
                    insomniaIntent(InsomniaReceiver.LOCK, n.id)).build())
                b.addAction(Notification.Action.Builder(null, context.getString(R.string.insomnia_fine),
                    insomniaIntent(InsomniaReceiver.FINE, n.id)).build())
                notifyTagged(keyOf(key), b.build())
                announced += key
                insomniaShown[key] = said
            }
        }
        for (w in i?.warnings.orEmpty().takeIf { i?.on == true }.orEmpty()) {
            val key = "iw:${w.id}"
            now[key] = keyOf(key)
            val known = key in announced
            if (known && insomniaShown[key] == null) insomniaShown[key] = w.text
            // the same warning with new words (the lid on battery follows the Mac's helper): updated quietly
            if (known && insomniaShown[key] == w.text) continue
            announced += key
            insomniaShown[key] = w.text
            val b = builder(CH_INSOMNIA, context.getString(R.string.n_insomnia_warning), w.text, open(null), public = R.string.n_public_mac)
            if (known) b.setOnlyAlertOnce(true).setGroup(SILENT_GROUP).setGroupAlertBehavior(Notification.GROUP_ALERT_SUMMARY)
            notifyTagged(keyOf(key), b.build())
        }
        // what the status no longer has goes (answered on the Mac or here, gone, the mode ended)
        for (sbn in nm.activeNotifications) {
            if (sbn.tag == TAG_INSOMNIA && sbn.id !in now.values) nm.cancel(TAG_INSOMNIA, sbn.id)
        }
        announced.removeAll { (it.startsWith("in:") || it.startsWith("iw:")) && it !in now }
        insomniaShown.keys.retainAll(now.keys)
        // how it ended: once, quietly; in the app the Mac's alert says it (kb APP-1109). Turned off by the owner himself:
        // nothing to tell him (kb APP-1108, as the Mac)
        val end = i?.lastEnd?.takeIf { !i.on && it.text.isNotEmpty() && it.why != "turned_off" }
        if (end != null && !quiet && prefs.claimInsomniaEnd(end.id)) {
            val b = builder(CH_DONE, context.getString(R.string.n_insomnia_warning), end.text, open(null))
                .setOnlyAlertOnce(true).setGroup(SILENT_GROUP).setGroupAlertBehavior(Notification.GROUP_ALERT_SUMMARY)
            notifyTagged(keyOf("ie:${end.id}"), b.build(), tag = TAG_INSOMNIA_END)
        }
    }

    /** What each insomnia notification said as last posted (its count and words): a change updates it quietly. */
    private val insomniaShown = ConcurrentHashMap<String, String>()

    @Volatile private var asleepPosted = false

    /** The Mac may be asleep (APP-981): one quiet notification while it can't be reached, gone when it is back. */
    private fun asleep(on: Boolean) {
        if (!on) {
            if (asleepPosted) nm.cancel(keyOf(KEY_ASLEEP))
            asleepPosted = false
            return
        }
        if (asleepPosted) return
        asleepPosted = true
        val b = builder(CH_INSOMNIA, context.getString(R.string.n_insomnia_warning), context.getString(R.string.mac_asleep), open(null),
            public = R.string.n_public_mac)
            .setOnlyAlertOnce(true).setGroup(SILENT_GROUP).setGroupAlertBehavior(Notification.GROUP_ALERT_SUMMARY)
        notify(keyOf(KEY_ASLEEP), b.build())
    }

    /** A notice's answer from here: gone at once on success (the status removes it too), else what stopped it. */
    fun insomniaAnswered(noticeId: String, problem: String?) {
        val id = keyOf("in:$noticeId")
        if (problem == null) { nm.cancel(TAG_INSOMNIA, id); return }
        notifyTagged(id, builder(CH_INSOMNIA, context.getString(R.string.n_not_answered_title), problem, open(null),
            public = R.string.n_public_mac).build())
    }

    /** Show me: the app opens on the camera view (after its own unlock, PHN-8). */
    private fun camera(): PendingIntent {
        val i = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra(MainActivity.EXTRA_CAMERA, true)
        return PendingIntent.getActivity(context, "camera".hashCode(), i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun insomniaIntent(action: String, noticeId: String): PendingIntent {
        val i = Intent(context, InsomniaReceiver::class.java).setAction(action).putExtra(InsomniaReceiver.NOTICE, noticeId)
        return PendingIntent.getBroadcast(context, "$action:$noticeId".hashCode(), i,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun notifyTagged(id: Int, n: Notification, tag: String = TAG_INSOMNIA) {
        if (context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        nm.notify(tag, id, n)
    }

    private fun cancel(approvalId: String) {
        nm.cancel(keyOf("q:$approvalId"))
        posted.remove(approvalId)
    }

    /**
     * The turns that finished or failed, by task id, each as the Mac's banner says it (kb PHN-795): the task's short
     * name and the answer in the chat's words, never a status word of the phone's own. A cancelled end has no banner on
     * the Mac and no notification here (his own stop, a question he declined, a restart's first turn): left out
     * unannounced, so a restarted task's real end is still news.
     */
    private fun finished(view: TranscriptView): List<Pair<String, NotificationWords>> =
        view.items.mapNotNull { (it as? Item.Turn)?.turn }
            .filter { it.taskId != null && it.answer != null && it.status !in setOf("working", "needs_user", "merged") }
            .mapNotNull { t ->
                val a = t.answer!!
                taskEndWords(t.status, t.taskName.orEmpty(), a.text, a.displayText, false)?.let { t.taskId!! to it }
            }

    /** A question's notification, worded and with buttons as its needs allow (the class comment). */
    /**
     * [quietly]: an update of one in the shade, or one he already saw in the app (no sound, no heads-up). A new one is
     * a heads-up on the questions channel (high importance), with sound and vibration as the phone's settings allow.
     */
    fun question(q: Question, lockedNow: Boolean = keyguard.isKeyguardLocked, quietly: Boolean = false) {
        val id = q.approvalId
        val wordless = q.ownCheck && prefs.hideChecksOnLockScreen && lockedNow
        val asking = headline(q) ?: context.getString(R.string.n_question_title)
        val said = detail(q)
        val (title, text) = when {
            q.hidden -> context.getString(R.string.n_private_title) to context.getString(R.string.n_private_text)
            q.held -> context.getString(R.string.n_question_title) to context.getString(R.string.n_held_text)
            wordless -> context.getString(R.string.n_check_title) to context.getString(R.string.n_check_text)
            // the planner's question (kb PHN-762): his answer is a message in the chat, which the tap opens
            q.clarify -> asking to listOfNotNull(said, context.getString(R.string.n_clarify_text)).joinToString("\n")
            q.onlyOnMac -> asking to listOfNotNull(said, context.getString(R.string.n_on_mac)).joinToString("\n")
            !q.routine -> asking to listOfNotNull(said, context.getString(when {
                q.needs == null -> R.string.n_needs_fingerprint_text
                q.needs.fingerprint -> R.string.n_needs_fingerprint
                else -> R.string.n_needs_unlock
            })).joinToString("\n")
            else -> asking to (said ?: asking)
        }
        val tap = if (q.clarify) open(CHAT) else open(null, id)
        val b = builder(if (q.ownCheck) CH_CHECKS else CH_QUESTIONS, title, text, tap).setOnlyAlertOnce(quietly)
        // no sound and no heads-up for one he already saw (or an update): as NotificationCompat.setSilent does it, a
        // group whose children never alert (it has no summary, so the notification shows as any other)
        if (quietly) b.setGroup(SILENT_GROUP).setGroupAlertBehavior(Notification.GROUP_ALERT_SUMMARY)
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

    /**
     * A notification's title and words for a question: the Mac's own banner words, from the library both phones share
     * (`questionWords`, ferb-mobile notify.rs, kb PHN-795; ferb-daemon notify.rs `approval_notice`: "FERB needs you",
     * "FERB needs your OK: …"), so a question reads the same in the Mac's banner and here. The planner's question has
     * no banner on the Mac (the notch opens): the same title and its own words. Without the Mac's presentation (an older
     * Mac) the question's title and reason. The one place a notification reads a question's words; the lock-screen and
     * held-words rules apply on top.
     */
    private fun headline(q: Question): String? =
        q.shown?.let { questionWords(it).title } ?: if (q.clarify) context.getString(R.string.n_needs_you) else q.title

    private fun detail(q: Question): String? = (q.shown?.let { questionWords(it).text } ?: q.reason).ifEmpty { null }

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

    private fun builder(channel: String, title: String, text: String, tap: PendingIntent, public: Int? = null): Notification.Builder {
        val public = Notification.Builder(context, channel).setSmallIcon(R.drawable.ic_stat_ferb)
            .setContentTitle(context.getString(R.string.app_name))
            .setContentText(context.getString(public ?: if (channel == CH_DONE) R.string.n_public_done else R.string.n_public_question))
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
            ch(CH_INSOMNIA, R.string.ch_insomnia, R.string.ch_insomnia_desc, NotificationManager.IMPORTANCE_HIGH),
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
        /** Insomnia mode (APP-981, APP-983): someone at the Mac, its battery, heat and cover. */
        const val CH_INSOMNIA = "your_mac"
        /** Insomnia mode's notifications carry this tag (each removed when the Mac's status no longer has it). */
        const val TAG_INSOMNIA = "insomnia"
        const val TAG_INSOMNIA_END = "insomnia_end"
        const val KEY_LINK = "link"
        const val KEY_LOCATION = "location"
        /** The Mac may be asleep while insomnia mode was on (APP-981). */
        const val KEY_ASLEEP = "mac_asleep"
        const val SILENT_GROUP = "silent"
        /** A tap that opens the chat (MainActivity.EXTRA_ENTRY): where the planner's question is answered. */
        const val CHAT = "chat"
        fun keyOf(key: String): Int = key.hashCode()

        /** The questions a status brings, for a test or a receiver: each with its needs. */
        fun questionsOf(status: PhoneStatus?): List<Question> = Questions.merged(emptyList(), status)
    }
}

/**
 * One run of [work] at a time on [scope]; asks that come while it runs make one more run after it, never a queue of
 * them. Asked from the scope's own thread (the main thread).
 */
class Coalesced(private val scope: CoroutineScope, private val work: suspend () -> Unit) {
    private var job: kotlinx.coroutines.Job? = null
    private var again = false

    fun ask() {
        if (job?.isActive == true) { again = true; return }
        job = scope.launch {
            do {
                again = false
                work()
            } while (again)
        }
    }
}
