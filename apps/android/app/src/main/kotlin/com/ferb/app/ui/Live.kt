package com.ferb.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.design.presenceFade
import com.ferb.app.model.LiveView
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.components.CloseButton
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.paddingOverBottom

/**
 * Watch live (PHN-454): the task's window on the Mac as FERB.app's preview shows it, full screen, with FERB's line
 * under it and Stop. The frame lives in memory only; the window is kept out of screenshots and recent apps while this
 * shows (MainActivity: FLAG_SECURE through the view model's mustSecure, whatever the owner's switch says).
 */
@Composable
fun LiveScreen(vm: FerbViewModel, insets: PhoneInsets) {
    val view by vm.live.view.collectAsState()
    val v = view ?: return
    BackHandler { vm.closeLive() }
    LivePage(v, insets, onClose = vm::closeLive, onStop = { vm.stop(v.taskId) },
        onAgain = if (v.ended != null) ({ vm.watchAgain() }) else null)
}

/** The page itself (drawn from a [LiveView]; the screenshots hold it). */
@Composable
fun LivePage(v: LiveView, insets: PhoneInsets, onClose: () -> Unit, onStop: (() -> Unit)?, onAgain: (() -> Unit)?, title: String? = null) {
    // opened over the page, it rises and fades in, and fades out (kb PHN-623)
    Column(Modifier.presenceFade().fillMaxSize().background(Color.Black).paddingOverBottom(insets, 8.dp, top = insets.top + 8.dp)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            PhoneText(title ?: stringResource(R.string.live_title), 22f, FerbType.Semibold, Ink.white, Modifier.weight(1f).semantics { heading() }, maxLines = 2)
            CloseButton(onClose)
        }
        Box(Modifier.weight(1f).fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
            val frame = v.frame
            if (frame != null) {
                val image = remember(frame) { frame.asImageBitmap() }
                Image(image, v.line ?: stringResource(R.string.live_title), Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
            } else {
                val text = v.ended?.let { noteText(it) } ?: v.empty ?: stringResource(R.string.live_waiting)
                PhoneText(text, 16f, color = Ink.onDark, modifier = Modifier.padding(horizontal = 16.dp).semantics { liveRegion = LiveRegionMode.Polite })
            }
        }
        val line = if (v.frame != null) v.ended?.let { noteText(it) } ?: v.line else v.line.takeIf { v.ended == null }
        if (line != null) {
            PhoneText(line, 15f, FerbType.Medium, Ink.onDark, Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp)
                .semantics { liveRegion = LiveRegionMode.Polite })
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (onAgain != null) Pill(stringResource(R.string.live_again), Modifier.weight(1f), PillKind.GREY, FerbIcons.eye, height = 46.dp, onClick = onAgain)
            else Pill(stringResource(R.string.live_close), Modifier.weight(1f), PillKind.GREY, height = 46.dp, onClick = onClose)
            if (onStop != null) Pill(stringResource(R.string.live_stop), Modifier.weight(1f), PillKind.GREY, FerbIcons.stop, height = 46.dp, onClick = onStop)
        }
    }
}
