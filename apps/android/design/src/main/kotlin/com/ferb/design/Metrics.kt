package com.ferb.design

import androidx.compose.ui.unit.dp

// Shape and space. Numbers are the lab's points (1 pt = 1 dp). The phone screens' own numbers come from
// Mobile.swift and MobileScreens.swift; tokens.json's radius and space are the Mac's book, kept for the glass pieces.

/** tokens.json `radius` (the design book's radius per role). */
object Radius {
    val chip = 6.dp
    val key = 5.5.dp
    val buttonCompact = 7.dp
    val button = 9.dp
    val list = 12.dp
    val card = 14.dp
    val bubble = 16.dp
    val window = 20.dp
    val notchBottom = 26.dp
    val phoneSheet = 34.dp
}

/** tokens.json `space`. */
object Space {
    val xs = 4.dp
    val s = 8.dp
    val m = 12.dp
    val l = 16.dp
    val xl = 24.dp
    val all = listOf(xs, s, m, l, xl)
}

/** The phone's own shapes and sizes (Mobile.swift, MobileScreens.swift, Conversation.swift). */
object Phone {
    /** Mobile.swift OS (android): the frame the lab draws, the status bar's height and the gesture area. */
    val frameWidth = 412.dp
    val frameHeight = 915.dp
    val frameCorner = 44.dp
    val frameBezel = 9.dp
    val statusBar = 44.dp
    val gestureArea = 24.dp

    /** Pages: the side margin and the gap above the bottom inset. */
    val pageMargin = 16.dp
    val bottomGap = 8.dp

    /** Card: a white card of rows, 28 corners, 6 inside. */
    val cardCorner = 28.dp
    val cardPadding = 6.dp
    /** CardRow: 14 across, 13 down, the leading slot 28 wide, 14 between. */
    val rowPaddingH = 14.dp
    val rowPaddingV = 13.dp
    val rowLeading = 28.dp
    val rowGap = 14.dp

    /**
     * TopBar (PHN-801, the owner: "i want teh names", "i like b"): a black pill of four tabs, 42 high inside a 4 margin.
     * The chosen tab shows its icon and name (padding 12, gap 6); the others share what is left beside the profile
     * button, the lab's 46 at most and never under 36. The connection's state is a badge on the profile button (no second
     * round button). The same on iPhone.
     */
    val tabHeight = 42.dp
    val tabMinWidth = 46.dp
    val tabNarrowest = 36.dp
    val tabPadding = 12.dp
    val tabNameGap = 6.dp
    val tabSpacing = 0.dp
    val tabBarPadding = 4.dp
    val barButtonGap = 8.dp
    val linkBadge = 14.dp
    val profileButton = 42.dp

    /** Bubbles: 22 corners with a 6 tail corner, 16 by 11 inside, at most 310 (owner) and 320 (FERB) wide. */
    val bubbleCorner = 22.dp
    val bubbleTail = 6.dp
    val bubbleMaxOwner = 310.dp
    val bubbleMaxFerb = 320.dp

    /** The task card: 26 corners, 8 inside; a running part on a 18-corner fill. */
    val taskCorner = 26.dp
    val partCorner = 18.dp

    /** Pill: 16 corners, 52 high (46 in the task card, 44 for Cancel). */
    val pillCorner = 16.dp
    val pillHeight = 52.dp

    /** Composer: a black capsule 56 high, round buttons 40. */
    val composerHeight = 56.dp
    val composerButton = 40.dp

    /** Sheet: 44 corners at the bottom (32 lifted over a keypad), inset 8, 22 inside, grabber 38 by 5. */
    val sheetCorner = 44.dp
    val sheetCornerLifted = 32.dp
    val sheetInset = 8.dp
    val sheetPadding = 22.dp
    val sheetIcon = 46.dp
    val sheetIconCorner = 15.dp
    val closeButton = 36.dp

    /** Mark: 26 across. */
    val mark = 26.dp

    /** CodesPage: three pairs of digits in 86 squares with 24 corners. */
    val codeBox = 86.dp
    val codeCorner = 24.dp

    /** SecretSheet: six boxes 54 high with 14 corners; keys 50 high with 12 corners. */
    val secretBoxHeight = 54.dp
    val secretBoxCorner = 14.dp
    val keyHeight = 50.dp
    val keyCorner = 12.dp

    /** Switch: 52 by 32 with a 24 knob. */
    val switchWidth = 52.dp
    val switchHeight = 32.dp

    /** The smallest touch target (Android's accessibility guidance), reached without changing what is drawn. */
    val touchTarget = 48.dp
}
