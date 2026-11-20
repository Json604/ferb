package com.ferb.app

import android.app.Notification
import android.app.PendingIntent
import android.app.RemoteInput
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import com.ferb.app.model.Questions
import com.ferb.core.Outcome
import com.ferb.core.PhoneState
import com.ferb.mobile.AnswerInput
import com.ferb.mobile.FerbException
import com.ferb.mobile.LinkPhase
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Stay connected (PHN-421): a foreground service that keeps the link up while the app is not on screen, so FERB's
 * questions, replies and finished tasks reach the phone as notifications. It holds no loop of its own: the link's
 * events drive everything (battery: no polling). With it on, the link survives the app being swiped away; with it
 * off, the link closes when the app leaves the screen and nothing arrives until it is opened again.
 *
 * Type remoteMessaging (Android 14+): it carries FERB's messages between the owner's Mac and this phone. dataSync was
 * not taken (Android 15 stops it after 6 hours a day), connectedDevice needs extra permissions and names hardware
 * peripherals, specialUse needs a store's review of a free-text reason. kb PHN-501.
 */
class StayConnectedService : Service() {
    override fun onCreate() {
        super.onCreate()
        val app = application as FerbApplication
        app.notifier.channels()
        val tap = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val note = Notification.Builder(this, Notifier.CH_STAY)
            .setSmallIcon(R.drawable.ic_stat_ferb)
            .setContentTitle(getString(R.string.stay_title))
            .setContentText(getString(R.string.stay_text))
            .setOngoing(true)
            .setContentIntent(tap)
            // shown at once: the owner just turned it on (Android defers a new service's line up to 10 s otherwise)
            .setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
        startForeground(ID, note, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
        running = true
        app.signals.stayConnected = true
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        running = false
        (application as FerbApplication).signals.stayConnected = false
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        const val ID = 1
        @Volatile var running = false
            private set

        /** Turned on or off from the app on screen (a foreground service may only be started from there). */
        fun set(context: Context, on: Boolean) {
            val i = Intent(context, StayConnectedService::class.java)
            if (on) context.startForegroundService(i) else context.stopService(i)
        }
    }
}

/** After an update of the app, Stay connected comes back by itself if it was on (no permission needed for this). */
class UpdatedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val app = context.applicationContext as FerbApplication
        if (app.prefs.stayConnected) runCatching { StayConnectedService.set(context, true) }
    }
}

/**
 * An answer given in a notification (PHN-415, PHN-417): sent at once, never queued, with no prompt (a routine Yes,
 * an ordinary answer and a plain No need none, PHN-470). When the Mac says this one needs the phone's own proof, the
 * notification says so and a tap opens the question in the app, where the fingerprint is asked (PHN-13).
 */
class AnswerReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(ID) ?: return
        val choice = intent.getIntExtra(CHOICE, -1)
        val typed = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(TEXT)?.toString()?.trim().orEmpty()
        val app = context.applicationContext as FerbApplication
        val pending = goAsync()
        app.scope.launch {
            try {
                // a process started for this answer opens the phone first (bounded: a receiver has seconds)
                withTimeoutOrNull(8_000) {
                    app.phone.state.first { it is PhoneState.Open }
                    app.phone.link.first { it?.phase == LinkPhase.ONLINE }
                }
                // the question as the chat holds it, else as the Mac's status gives it (a locked app reads no chat)
                val status = app.api.lastStatus() ?: (app.api.status() as? Outcome.Ok)?.value
                val view = (app.api.transcript() as? Outcome.Ok)?.value
                val fromChat = view?.questions?.firstOrNull { it.approvalId == id && it.open }?.let(Questions::parse)
                val q = Questions.merged(listOfNotNull(fromChat), status).firstOrNull { it.approvalId == id }
                if (q == null || q.hidden) { app.notifier.answered(id, context.getString(R.string.question_gone), false); return@launch }
                // a button is only on a routine question; one that needs the phone's proof is answered in the app
                if (!q.routine) { app.notifier.answered(id, context.getString(R.string.n_needs_fingerprint_text), true); return@launch }
                val input: AnswerInput = when {
                    choice == UNTIL_BACK -> if (q.untilBack) AnswerInput.UntilBack else return@launch
                    choice >= 0 -> q.choices.getOrNull(choice)?.input ?: return@launch
                    typed.isNotEmpty() -> AnswerInput.Words(typed)
                    else -> return@launch
                }
                when (val r = app.api.answer(id, input, null)) {
                    is Outcome.Ok -> app.notifier.answered(id, null, false)
                    is Outcome.Failed -> {
                        val e = r.error
                        // "until I'm back" needs the app opened (its unlock), not a fingerprint: said so
                        if (e is FerbException.NeedsUnlock && input is AnswerInput.UntilBack) app.notifier.answered(id, context.getString(R.string.n_until_back_open), false)
                        else if (e is FerbException.NeedsUnlock) app.notifier.answered(id, context.getString(R.string.n_needs_fingerprint_text), true)
                        else app.notifier.answered(id, FerbViewModel.errorNote(e).let { n -> context.getString(n.res, n.words.orEmpty()) }, false)
                    }
                }
            } finally {
                pending.finish()
            }
        }
    }

    companion object {
        const val ACTION = "com.ferb.app.ANSWER"
        const val ID = "approval_id"
        const val CHOICE = "choice"
        const val TEXT = "answer"

        /** The notification's "Yes, until I'm back at my Mac" (not an index into the question's choices). */
        const val UNTIL_BACK = -2
    }
}

/**
 * Lock my Mac and It's fine from the insomnia notice (APP-983): sent at once, never queued, with no prompt (both are
 * plain calls the Mac takes while the phone is locked: they only take something away or say he saw it). Show me opens
 * the app instead (its unlock, then the camera view). What stopped one is said in the notification (PHN-302).
 */
class InsomniaReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(NOTICE) ?: return
        val action = intent.action ?: return
        if (action != LOCK && action != FINE) return
        val app = context.applicationContext as FerbApplication
        val pending = goAsync()
        app.scope.launch {
            try {
                // a process started for this answer opens the phone first (bounded: a receiver has seconds)
                withTimeoutOrNull(8_000) {
                    app.phone.state.first { it is PhoneState.Open }
                    app.phone.link.first { it?.phase == LinkPhase.ONLINE }
                }
                val r = if (action == LOCK) app.api.lockMac() else app.api.dismissInsomniaNotice(id)
                when (r) {
                    is Outcome.Ok -> app.notifier.insomniaAnswered(id, null)
                    is Outcome.Failed -> app.notifier.insomniaAnswered(id,
                        FerbViewModel.errorNote(r.error).let { n -> context.getString(n.res, n.words.orEmpty()) })
                }
            } finally {
                pending.finish()
            }
        }
    }

    companion object {
        const val LOCK = "com.ferb.app.INSOMNIA_LOCK"
        const val FINE = "com.ferb.app.INSOMNIA_FINE"
        const val NOTICE = "notice_id"
    }
}
