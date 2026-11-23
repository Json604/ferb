package com.ferb.design.components

import androidx.compose.ui.unit.Dp
import com.ferb.design.FerbType
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.foundation.layout.BoxWithConstraints

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
import androidx.compose.ui.platform.testTag
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
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
private data class BarValue(val chosen: PhoneTab, val waiting: PhoneTab?, val onTab: ((PhoneTab) -> Unit)?, val onProfile: (() -> Unit)?,
                            val temporary: TemporaryToggle? = null)

private class TabBarState {
    var bar by mutableStateOf<BarValue?>(null)
    /** The folded bar's menu is open (PHN-820): drawn by the host over every page. */
    val menu = mutableStateOf(false)
}

/** The folded bar's menu, open or not: the host's, so its panel and the tap that closes it cover the pages. */
private val LocalBarMenu = staticCompositionLocalOf<androidx.compose.runtime.MutableState<Boolean>?> { null }

/** What the folded bar's menu offers (PHN-820), handed to the host that draws it. */
private class MenuValue(val temporary: TemporaryToggle, val onProfile: (() -> Unit)?, val connection: String?, val sideEnd: Dp)

private val LocalBarMenuValue = staticCompositionLocalOf<androidx.compose.runtime.MutableState<MenuValue?>?> { null }

/**
 * The bar's left and right insets (the display cutout and the system bars, PHN-820): read from the window unless given
 * here (a golden's stand-in for a phone with a notch at the side).
 */
data class BarSides(val start: Dp, val end: Dp)

val LocalBarSides = staticCompositionLocalOf<BarSides?> { null }

/** How the bar fits its width (PHN-820), in the order things give way. */
enum class BarFit {
    /** Every tab (unchosen ones at least 36), the temporary chat's bubble on the chat tab, the profile button. */
    NORMAL,
    /** The bubble and the profile button folded into one menu button; the tabs as in NORMAL. */
    FOLDED,
    /** Folded, and the chosen tab shows only its icon in the white pill. */
    CONTRACTED,
}

/** The bar as it fits: how, each unchosen tab's width, whether the chosen tab shows its name, whether there is a menu. */
data class BarLayout(val fit: BarFit, val tabWidth: Dp, val showName: Boolean, val menu: Boolean)

/**
 * The order of giving way (owner 2026-11-23, PHN-820), a pure function of the bar's room ([barWidth]: the window less
 * its side insets and the bar's own margins), the chosen tab's name as measured at the font scale ([nameWidth]) and the
 * round buttons it would carry ([buttons]: 2 on the chat tab with the bubble, else 1): normal while every unchosen tab
 * keeps 36 (PHN-801's narrowest); else the two round buttons fold into one menu button; else the chosen tab drops its
 * name. One round button alone never folds (a menu of one saves nothing): it goes straight to dropping the name.
 */
fun barLayout(barWidth: Dp, nameWidth: Dp, buttons: Int): BarLayout {
    val others = PhoneTab.entries.size - 1
    val named = Phone.tabPadding * 2 + 17.dp + Phone.tabNameGap + nameWidth
    val iconOnly = Phone.tabPadding * 2 + 17.dp
    fun tabs(round: Int, chosen: Dp) = (barWidth - Phone.tabBarPadding * 2 - (Phone.profileButton + Phone.barButtonGap) * round - chosen) / others
    val normal = tabs(buttons, named)
    if (normal >= Phone.tabNarrowest) return BarLayout(BarFit.NORMAL, normal.coerceAtMost(Phone.tabMinWidth), true, false)
    if (buttons > 1) {
        val folded = tabs(1, named)
        if (folded >= Phone.tabNarrowest) return BarLayout(BarFit.FOLDED, folded.coerceAtMost(Phone.tabMinWidth), true, true)
    }
    // the icon (17) always keeps a little room around it, however narrow
    val contracted = tabs(1, iconOnly).coerceIn(21.dp, Phone.tabMinWidth)
    return BarLayout(BarFit.CONTRACTED, contracted, false, buttons > 1)
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
        val menuValue = remember { mutableStateOf<MenuValue?>(null) }
        state.bar?.let { b ->
            CompositionLocalProvider(LocalTabBar provides null, LocalMotionPresence provides null, LocalBarMenu provides state.menu,
                LocalBarMenuValue provides menuValue) {
                TopBar(b.chosen, waiting = b.waiting, onTab = b.onTab ?: onTab, onProfile = b.onProfile ?: onProfile, temporary = b.temporary)
            }
        }
        val m = menuValue.value
        if (state.menu.value && m != null) {
            // a tap anywhere else closes it
            Box(Modifier.fillMaxSize().pressable({ state.menu.value = false }).testTag("topbar-menu-scrim"))
            BarMenu(m, Modifier.align(Alignment.TopEnd).padding(top = 6.dp + Phone.profileButton + 8.dp, end = 16.dp + m.sideEnd)) { state.menu.value = false }
        }
    }
}

/**
 * Mobile.swift TopBar: the top of every page, a short black pill of tabs (the chosen one shows its name, PHN-801) and
 * the profile button with the link's state as its badge. [waiting] puts an amber dot on a tab that is not chosen. Inside a [TabBarHost] it only keeps its
 * room and gives its tabs to the host's bar. Not from the lab: [temporary], the temporary chat's dashed bubble beside the
 * profile button (the chat tab only; it takes its 42 and a gap from the tabs' room, as on the iPhone).
 */
@Composable
fun TopBar(
    chosen: PhoneTab,
    modifier: Modifier = Modifier,
    waiting: PhoneTab? = PhoneTab.TASKS,
    onTab: ((PhoneTab) -> Unit)? = null,
    onProfile: (() -> Unit)? = null,
    temporary: TemporaryToggle? = null,
) {
    val host = LocalTabBar.current
    if (host != null) {
        if (LocalMotionPresence.current?.leaving != true) SideEffect { host.bar = BarValue(chosen, waiting, onTab, onProfile, temporary) }
        Box(modifier.alpha(0f).clearAndSetSemantics { }) { Bar(chosen, Modifier, null, null, null, null) }
        return
    }
    Bar(chosen, modifier, waiting, onTab, onProfile, temporary)
}

/** Where the white pill is (MotionHosts' tab spring): the chosen tab's place, reached by a spring that may overshoot. */
private class Pill {
    val cells = mutableStateListOf<Rect?>().apply { repeat(PhoneTab.entries.size) { add(null) } }
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
    temporary: TemporaryToggle? = null,
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
    // decision: PHN-801 — the chosen tab's name in the pill; the tabs share what is left (the lab's 46 at most); the
    // link's state is a badge on the profile button, whose page holds the connection
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val conn = LocalConnection.current
    // decision: PHN-820 — the room is the window less its side insets (a cutout, the system bars) and the bar's margins
    val sides = LocalBarSides.current ?: run {
        val ins = WindowInsets.displayCutout.union(WindowInsets.systemBars)
        val dir = androidx.compose.ui.platform.LocalLayoutDirection.current
        with(density) { BarSides(ins.getLeft(this, dir).toDp(), ins.getRight(this, dir).toDp()) }
    }
    BoxWithConstraints(modifier.fillMaxWidth().padding(start = 16.dp + sides.start, end = 16.dp + sides.end, top = 6.dp)) {
        val name = with(density) { measurer.measure(chosen.title, FerbType.style(15f, FerbType.Semibold)).size.width.toDp() }
        val fit = barLayout(maxWidth, name, if (temporary != null) 2 else 1)
        val tabW = fit.tabWidth
        val menuOpen = LocalBarMenu.current
        val menuValue = LocalBarMenuValue.current
        if (fit.menu && temporary != null) {
            val value = MenuValue(temporary, onProfile, conn?.label, sides.end)
            SideEffect { menuValue?.value = value }
        } else {
            SideEffect { menuValue?.value = null; menuOpen?.value = false }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Row(
                Modifier.background(Ink.black, CircleShape).drawBehind { drawPill(pill, chosen) }.padding(Phone.tabBarPadding).selectableGroup(),
                horizontalArrangement = Arrangement.spacedBy(Phone.tabSpacing),
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
                                .then(if (on) Modifier.padding(horizontal = Phone.tabPadding) else Modifier.width(tabW))
                                .testTag(if (on) "tab-chosen" else "tab-${tab.name.lowercase()}")
                                .heightIn(min = Phone.tabHeight),
                            horizontalArrangement = Arrangement.spacedBy(Phone.tabNameGap, Alignment.CenterHorizontally),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            FerbIcon(if (on) tab.chosenIcon else tab.icon, 17f, ink)
                            if (on && fit.showName) PhoneText(tab.title, 15f, FerbType.Semibold, ink, maxLines = 1)
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
            if (fit.menu && temporary != null) {
                MenuButton(temporary, conn, open = menuOpen?.value == true) { menuOpen?.let { it.value = !it.value } }
                return@Row
            }
            if (temporary != null) {
                TemporaryButton(temporary, Modifier.testTag("temporary-chat"))
                Spacer(Modifier.width(Phone.barButtonGap))
            }
            val label = if (conn != null) "This phone. ${conn.label}" else "This phone"
            Box(
                Modifier.pressable(onProfile, label = label).semantics { contentDescription = label }
                    .size(Phone.profileButton).background(Ink.darkFill, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                FerbIcon(FerbIcons.person, 17f, Ink.white)
                // the link's state as a badge on the profile button (PHN-801; its sheet is a row on the profile page)
                if (conn != null) {
                    Box(
                        Modifier.align(Alignment.BottomEnd).offset((-2).dp, (-2).dp).size(Phone.linkBadge)
                            .background(Ink.darkFill, CircleShape),
                        contentAlignment = Alignment.Center,
                    ) { LinkMarkView(conn.mark, 7.dp) }
                }
            }
        }
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

/**
 * The folded bar's one round button (PHN-820): three lines, 18 wide and 2 thick with round caps, 5 apart, centred, white
 * on the dark circle; the link's badge where the profile button has it; while the temporary chat is on, the dashed
 * bubble's mark at its top left (on white, as the bubble's own on state), so the on state always shows.
 */
@Composable
private fun MenuButton(temporary: TemporaryToggle, conn: Connection?, open: Boolean, onClick: () -> Unit) {
    val label = if (conn != null) "${temporary.more}. ${conn.label}" else temporary.more
    Box(
        Modifier.pressable(onClick, label = label).semantics { contentDescription = label; selected = open }
            .size(Phone.profileButton).background(Ink.darkFill, CircleShape).testTag("topbar-menu"),
        contentAlignment = Alignment.Center,
    ) {
        androidx.compose.foundation.Canvas(Modifier.size(18.dp)) { drawHamburger() }
        if (temporary.on) {
            Box(
                Modifier.align(Alignment.TopStart).offset(1.dp, 1.dp).size(16.dp).background(Ink.white, CircleShape).testTag("topbar-menu-temporary"),
                contentAlignment = Alignment.Center,
            ) { DashedBubble(true, size = 12.dp) }
        }
        if (conn != null) {
            Box(
                Modifier.align(Alignment.BottomEnd).offset((-2).dp, (-2).dp).size(Phone.linkBadge).background(Ink.darkFill, CircleShape),
                contentAlignment = Alignment.Center,
            ) { LinkMarkView(conn.mark, 7.dp) }
        }
    }
}

/** Three lines 18 wide, 2 thick, round caps, 5 apart (a 5 gap between them: 16 tall), centred in an 18 box. */
private fun DrawScope.drawHamburger() {
    val w = 2.dp.toPx()
    val step = 7.dp.toPx()
    val cy = size.height / 2
    val half = w / 2
    for (i in -1..1) {
        val y = cy + i * step
        drawLine(Ink.white, androidx.compose.ui.geometry.Offset(half, y), androidx.compose.ui.geometry.Offset(size.width - half, y), w,
            cap = androidx.compose.ui.graphics.StrokeCap.Round)
    }
}

/**
 * The folded bar's menu (PHN-820): a dark panel under the menu button, right-aligned: the temporary chat (its bubble,
 * its words as the bubble's label says them, a check while on), then this phone (its icon, "This phone" and the link's
 * state), which opens the profile page as the profile button does.
 */
@Composable
private fun BarMenu(m: MenuValue, modifier: Modifier, close: () -> Unit) {
    val t = m.temporary
    androidx.compose.foundation.layout.Column(
        modifier.widthIn(min = 240.dp, max = 300.dp).background(Ink.black, androidx.compose.foundation.shape.RoundedCornerShape(20.dp))
            .padding(6.dp).testTag("topbar-menu-panel"),
    ) {
        Row(
            Modifier.fillMaxWidth().pressable(if (t.busy) null else ({ close(); t.onClick() }), label = t.label)
                .semantics { contentDescription = t.label; selected = t.on }.heightIn(min = 48.dp).padding(horizontal = 12.dp)
                .testTag("topbar-menu-temporary-row"),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(22.dp), contentAlignment = Alignment.Center) { DashedBubble(false, size = 20.dp) }
            PhoneText(t.label, 16f, FerbType.Medium, Ink.white, maxLines = 1, modifier = Modifier.weight(1f))
            if (t.on) FerbIcon(FerbIcons.check, 18f, Ink.white)
        }
        Box(Modifier.fillMaxWidth().padding(horizontal = 12.dp).height(1.dp).background(Ink.white.copy(alpha = 0.12f)))
        val label = if (m.connection != null) "This phone. ${m.connection}" else "This phone"
        Row(
            Modifier.fillMaxWidth().pressable(m.onProfile?.let { f -> { close(); f() } }, label = label)
                .semantics(mergeDescendants = true) { contentDescription = label }.heightIn(min = 52.dp).padding(horizontal = 12.dp)
                .testTag("topbar-menu-profile-row"),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(22.dp), contentAlignment = Alignment.Center) { FerbIcon(FerbIcons.person, 17f, Ink.white) }
            androidx.compose.foundation.layout.Column(Modifier.weight(1f)) {
                PhoneText("This phone", 16f, FerbType.Medium, Ink.white, maxLines = 1)
                m.connection?.let { PhoneText(it, 13f, color = Ink.onDark, maxLines = 1) }
            }
        }
    }
}
