package com.ferb.gallery

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.ferb.design.FerbIcons
import com.ferb.design.MotionKind
import com.ferb.design.MotionSwitch
import com.ferb.design.components.ExactCommand
import com.ferb.design.components.LinkKind
import com.ferb.design.components.PhoneTab
import com.ferb.design.components.TabBarHost
import com.ferb.design.preview.SampleWork
import com.ferb.design.screens.Transfer
import com.ferb.design.preview.labChat
import com.ferb.design.preview.labHomeTexts
import com.ferb.design.preview.labPlaces
import com.ferb.design.preview.labRecent
import com.ferb.design.preview.labRelay
import com.ferb.design.preview.labTasks
import com.ferb.design.screens.ChatPage
import com.ferb.design.screens.FilesPage
import com.ferb.design.screens.HomePage
import com.ferb.design.screens.MacPage
import com.ferb.design.screens.PhoneInsets
import com.ferb.design.screens.PhoneScreen
import com.ferb.design.screens.ProfileSheet
import com.ferb.design.screens.QuestionSheet
import com.ferb.design.screens.RelaySheet
import com.ferb.design.screens.TasksPage
import com.ferb.design.sky.SkyMoment

/**
 * The app's motion (kb PHN-620..624) over the lab's pages, with the same hosts the app uses: tap a tab, a row of what
 * happened while away (the question), the link picture (the relay sheet) or the profile button; drag a sheet down.
 * Invented data only; nothing here reaches a Mac.
 */
@Composable
fun MotionDemo(moment: SkyMoment, insets: PhoneInsets) {
    var tab by remember { mutableStateOf(PhoneTab.CHAT) }
    var home by remember { mutableStateOf(true) }
    var sheet by remember { mutableStateOf<String?>(null) }
    val close = { sheet = null }
    val choose: (PhoneTab) -> Unit = { if (it == PhoneTab.CHAT && tab == PhoneTab.CHAT) home = !home; tab = it }
    PhoneScreen(moment, insets = insets, season = tab == PhoneTab.CHAT && home, sheet = {
        MotionSwitch(sheet, Modifier.fillMaxSize(), MotionKind.SHEET) { s ->
            when (s) {
                "question" -> QuestionSheet(FerbIcons.terminal, "Run this in Terminal?", "To update Figma before opening the file you asked for.",
                    "Yes, run it", insets, "1 of 2", onNo = close, onYes = close, onDismiss = close) { ExactCommand("brew upgrade --cask figma") }
                "relay" -> RelaySheet(labRelay, insets, onClose = close)
                else -> ProfileSheet("MacBook Pro", "10 November", insets, onClose = close)
            }
        }
    }) {
        TabBarHost(Modifier.fillMaxSize(), onTab = choose, onProfile = { sheet = "profile" }) {
            MotionSwitch(tab to home, Modifier.fillMaxSize(), MotionKind.TAB, direction = { a, b -> b.first.ordinal.compareTo(a.first.ordinal) }) { (t, h) ->
                Box(Modifier.fillMaxSize()) {
                    when (t) {
                        PhoneTab.CHAT -> if (h) HomePage(labHomeTexts(moment), SampleWork.away, insets, onTab = choose, onProfile = { sheet = "profile" },
                            onRow = { sheet = "question" }, onMore = { tab = PhoneTab.TASKS })
                        else ChatPage(labChat, insets)
                        PhoneTab.TASKS -> TasksPage(labTasks, onTab = choose, onProfile = { sheet = "profile" }, onRow = { _, _ -> sheet = "question" })
                        PhoneTab.FILES -> FilesPage(labPlaces, labRecent, Transfer("Sending 2 photos to your Mac", 0.64f), insets)
                        PhoneTab.MAC -> MacPage(LinkKind.RELAY, onTab = choose, onProfile = { sheet = "profile" }, onLink = { sheet = "relay" })
                    }
                }
            }
        }
    }
}
