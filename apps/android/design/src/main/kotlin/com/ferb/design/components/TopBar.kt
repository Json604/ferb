package com.ferb.design.components

import androidx.compose.ui.unit.Dp

import androidx.compose.foundation.layout.width

import androidx.compose.foundation.border

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.Animatable
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
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.addOutline
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.layout.boundsInParent
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.semantics.clearAndSetSemantics
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
import com.ferb.design.LocalMotionPresence
import com.ferb.design.Motion
import com.ferb.design.rememberReducedMotion
import kotlinx.coroutines.launch
import com.ferb.design.Phone

/** Mobile.swift Tab: the four pages. */
enum class PhoneTab(val title: String, val icon: ImageVector, val chosenIcon: ImageVector) {
    CHAT("Chat", FerbIcons.bubble, FerbIcons.bubbleFill),
    TASKS("Tasks", FerbIcons.checklist, FerbIcons.checklist),
    FILES("Files", FerbIcons.folder, FerbIcons.folderFill),
    MAC("Mac", FerbIcons.laptop, FerbIcons.laptop),
}

/**
 * How the link to the Mac stands, as the top bar's connection control shows it (kb PHN-751): connecting (the design's
 * turning arc, as FERB.app shows "Connecting to FERB…"), connected by its path (a quiet solid dot: white directly, white
 * with a ring through his own relay, grey through the shared relay), or can't reach (an amber lamp, only after
 * connecting has really failed).
 */
enum class LinkMark { CONNECTING, DIRECT, OWN_RELAY, SHARED_RELAY, UNREACHABLE }

/** The connection control: its [mark], what a screen reader says ([label]) and its tap (the connection sheet). */
data class Connection(val mark: LinkMark, val label: String, val onClick: (() -> Unit)?)

/** The connection control every page's top bar shows beside the profile button (kb PHN-751); none: no control. */
val LocalConnection = staticCompositionLocalOf<Connection?> { null }

/** What a page's own TopBar hands the one bar of a [TabBarHost]. */
private data class BarValue(val chosen: PhoneTab, val waiting: PhoneTab?, val onTab: ((PhoneTab) -> Unit)?, val onProfile: (() -> Unit)?)

private class TabBarState {
    var bar by mutableStateOf<BarValue?>(null)
}

private val LocalTabBar = staticCompositionLocalOf<TabBarState?> { null }

/**
 * One tab bar that stays while the pages under it change (kb PHN-622): each page's own TopBar keeps its room, draws
 * nothing and hands its tabs here; this draws the bar once, over them, so its white pill slides from tab to tab
 * (PairIt's bar). A page on its way out hands nothing.
 */
@Composable
fun TabBarHost(
    modifier: Modifier = Modifier,
    /** For a page that gives no tab callbacks of its own (the gallery's lab pages): the host's own. */
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    content: @Composable () -> Unit,
) {
    val state = remember { TabBarState() }
    Box(modifier) {
        CompositionLocalProvider(LocalTabBar provides state, content = content)
        state.bar?.let { b ->
            CompositionLocalProvider(LocalTabBar provides null, LocalMotionPresence provides null) {
                TopBar(b.chosen, waiting = b.waiting, onTab = b.onTab ?: onTab, onProfile = b.onProfile ?: onProfile)
            }
        }
    }
}

/**
 * Mobile.swift TopBar: the top of every page, a short black pill of tabs (only the chosen one shows its name) and the
 * profile button. [waiting] puts an amber dot on a tab that is not chosen. Inside a [TabBarHost] it only keeps its
 * room and gives its tabs to the host's bar.
 */
@Composable
fun TopBar(
    chosen: PhoneTab,
    modifier: Modifier = Modifier,
    waiting: PhoneTab? = PhoneTab.TASKS,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
) {
    val host = LocalTabBar.current
    if (host != null) {
        if (LocalMotionPresence.current?.leaving != true) SideEffect { host.bar = BarValue(chosen, waiting, onTab, onProfile) }
        Box(modifier.alpha(0f).clearAndSetSemantics { }) { Bar(chosen, Modifier, null, null, null) }
        return
    }
    Bar(chosen, modifier, waiting, onTab, onProfile)
}

/** Where the white pill is (MotionHosts' tab spring): the chosen tab's place, reached by a spring that may overshoot. */
private class Pill {
    val cells = mutableStateListOf<Rect?>(null, null, null, null)
    val x = Animatable(0f)
    val w = Animatable(0f)
    var placed by mutableStateOf(false)
}

@Composable
private fun Bar(
    chosen: PhoneTab,
    modifier: Modifier,
    waiting: PhoneTab?,
    onTab: ((PhoneTab) -> Unit)?,
    onProfile: (() -> Unit)?,
) {
    val pill = remember { Pill() }
    val reduced = rememberReducedMotion()
    val now by rememberUpdatedState(chosen)
    LaunchedEffect(pill) {
        snapshotFlow { pill.cells[now.ordinal] }.collect { target ->
            if (target == null) return@collect
            if (!pill.placed || reduced) {
                pill.x.snapTo(target.left)
                pill.w.snapTo(target.width)
                pill.placed = true
            } else {
                launch { pill.x.animateTo(target.left, Motion.tab.spec()) }
                launch { pill.w.animateTo(target.width, Motion.tab.spec()) }
            }
        }
    }
    Row(modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            Modifier.background(Ink.black, CircleShape).drawBehind { drawPill(pill, chosen) }.padding(Phone.tabBarPadding).selectableGroup(),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            for (tab in PhoneTab.entries) {
                val on = tab == chosen
                val dot = waiting == tab && !on
                val ink by animateColorAsState(if (on) Ink.black else Ink.white, Motion.enter(), label = "tab ink")
                Box(Modifier.onPlaced { c ->
                    val r = c.boundsInParent()
                    if (pill.cells[tab.ordinal] != r) pill.cells[tab.ordinal] = r
                }) {
                    Row(
                        Modifier
                            .pressable(onTab?.let { { it(tab) } }, role = Role.Tab)
                            .semantics(mergeDescendants = true) {
                                selected = on
                                contentDescription = if (dot) "${tab.title}, something is waiting" else tab.title
                            }
                            .animateContentSize(Motion.enter())
                            .widthIn(min = Phone.tabMinWidth).heightIn(min = Phone.tabHeight)
                            .padding(horizontal = if (on) Phone.tabPaddingOn else Phone.tabPaddingOff),
                        horizontalArrangement = Arrangement.spacedBy(7.dp, Alignment.CenterHorizontally),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        FerbIcon(if (on) tab.chosenIcon else tab.icon, 17f, ink)
                        if (on) PhoneText(tab.title, 15f, FerbType.Semibold, ink, maxLines = 1)
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
        LocalConnection.current?.let { c ->
            ConnectionButton(c)
            Spacer(Modifier.width(8.dp))
        }
        Box(
            Modifier.pressable(onProfile, label = "This phone").semantics { contentDescription = "This phone" }
                .size(Phone.profileButton).background(Ink.darkFill, CircleShape),
            contentAlignment = Alignment.Center,
        ) { FerbIcon(FerbIcons.person, 17f, Ink.white) }
    }
}

/**
 * The white pill under the chosen tab, where its spring has it; what overshoots the bar's ends is cut by the bar's own
 * round ends (PairIt's iPhone bar, ReceiverView.swift:322: `.clipShape(Capsule())`). Before the spring has a place it is
 * drawn at the chosen tab, so the first frame is never without it.
 */
private fun DrawScope.drawPill(pill: Pill, chosen: PhoneTab) {
    val target = pill.cells[chosen.ordinal] ?: return
    val pad = Phone.tabBarPadding.toPx()
    val x = if (pill.placed) pill.x.value else target.left
    val w = if (pill.placed) pill.w.value else target.width
    val pillOutline = { drawOutline(CircleShape.createOutline(Size(w, target.height), layoutDirection, this), Ink.white) }
    if (x >= 0f && x + w <= size.width - 2 * pad) {
        translate(pad + x, pad + target.top) { pillOutline() }
    } else {
        val bar = Path().apply { addOutline(CircleShape.createOutline(size, layoutDirection, this@drawPill)) }
        clipPath(bar) { translate(pad + x, pad + target.top) { pillOutline() } }
    }
}

/** The connection control (kb PHN-751): the profile button's round, with the link's mark in it. */
@Composable
fun ConnectionButton(c: Connection, modifier: Modifier = Modifier) {
    Box(
        modifier.pressable(c.onClick, label = c.label).semantics { contentDescription = c.label }
            .size(Phone.profileButton).background(Ink.darkFill, CircleShape),
        contentAlignment = Alignment.Center,
    ) { LinkMarkView(c.mark, 9.dp) }
}

/** The link's mark at [size] (the dot's size): the arc, a dot, a dot in a ring, or the amber lamp. */
@Composable
fun LinkMarkView(mark: LinkMark, size: Dp, modifier: Modifier = Modifier) {
    when (mark) {
        LinkMark.CONNECTING -> RunningArc(size * 1.8f, Ink.onDark, modifier)
        LinkMark.DIRECT -> Box(modifier.size(size).background(Ink.white, CircleShape))
        LinkMark.OWN_RELAY -> Box(modifier.size(size * 1.9f).border(1.2.dp, Ink.white.copy(alpha = 0.55f), CircleShape), contentAlignment = Alignment.Center) {
            Box(Modifier.size(size * 0.9f).background(Ink.white, CircleShape))
        }
        LinkMark.SHARED_RELAY -> Box(modifier.size(size).background(Ink.onDark, CircleShape))
        LinkMark.UNREACHABLE -> LampDot(Ink.amber, size, modifier)
    }
}
