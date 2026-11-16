package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone

/** Mobile.swift Tab: the four pages. */
enum class PhoneTab(val title: String, val icon: ImageVector, val chosenIcon: ImageVector) {
    CHAT("Chat", FerbIcons.bubble, FerbIcons.bubbleFill),
    TASKS("Tasks", FerbIcons.checklist, FerbIcons.checklist),
    FILES("Files", FerbIcons.folder, FerbIcons.folderFill),
    MAC("Mac", FerbIcons.laptop, FerbIcons.laptop),
}

/**
 * Mobile.swift TopBar: the top of every page, a short black pill of tabs (only the chosen one shows its name) and the
 * profile button. [waiting] puts an amber dot on a tab that is not chosen.
 */
@Composable
fun TopBar(
    chosen: PhoneTab,
    modifier: Modifier = Modifier,
    waiting: PhoneTab? = PhoneTab.TASKS,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
) {
    Row(modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            Modifier.background(Ink.black, CircleShape).padding(Phone.tabBarPadding).selectableGroup(),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            for (tab in PhoneTab.entries) {
                val on = tab == chosen
                val dot = waiting == tab && !on
                Box {
                    Row(
                        Modifier
                            .pressable(onTab?.let { { it(tab) } }, role = Role.Tab)
                            .semantics(mergeDescendants = true) {
                                selected = on
                                contentDescription = if (dot) "${tab.title}, something is waiting" else tab.title
                            }
                            .widthIn(min = Phone.tabMinWidth).heightIn(min = Phone.tabHeight)
                            .then(if (on) Modifier.background(Ink.white, CircleShape) else Modifier)
                            .padding(horizontal = if (on) Phone.tabPaddingOn else Phone.tabPaddingOff),
                        horizontalArrangement = Arrangement.spacedBy(7.dp, Alignment.CenterHorizontally),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        FerbIcon(if (on) tab.chosenIcon else tab.icon, 17f, if (on) Ink.black else Ink.white)
                        if (on) PhoneText(tab.title, 15f, FerbType.Semibold, Ink.black, maxLines = 1)
                    }
                    if (dot) {
                        // An 8 amber dot with a 2-wide black ring centred on its edge, offset (-8, 8) from the corner.
                        Box(
                            Modifier.align(Alignment.TopEnd).offset((-7).dp, 7.dp).size(10.dp)
                                .background(Ink.black, CircleShape).padding(2.dp).background(Ink.amber, CircleShape),
                        )
                    }
                }
            }
        }
        Spacer(Modifier.weight(1f))
        Box(
            Modifier.pressable(onProfile, label = "This phone").semantics { contentDescription = "This phone" }
                .size(Phone.profileButton).background(Ink.darkFill, CircleShape),
            contentAlignment = Alignment.Center,
        ) { FerbIcon(FerbIcons.person, 17f, Ink.white) }
    }
}
