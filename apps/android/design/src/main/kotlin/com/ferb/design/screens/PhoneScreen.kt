package com.ferb.design.screens

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.layout.layout
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.offset
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Motion
import com.ferb.design.Phone
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.PhoneText
import com.ferb.design.glass.LocalSkyBackdrop
import com.ferb.design.glass.SkyBackdrop
import com.ferb.design.glass.skyBackdropHost
import com.ferb.design.sky.SeasonLayer
import com.ferb.design.sky.Sky
import com.ferb.design.sky.SkyMoment

/**
 * The system's bars: the lab's os.top and os.bottom on Android. The app passes the real window insets: its bottom is
 * [live] (the keyboard moves it on every frame of its slide), read only where it is used, so a moving keyboard
 * recomposes only what reads [bottom] in composition, and nothing where it is read while measuring
 * ([paddingOverBottom]) (kb PHN-712).
 */
@Stable
class PhoneInsets private constructor(val top: Dp, private val fixed: Dp, private val live: (() -> Dp)?) {
    constructor(top: Dp, bottom: Dp) : this(top, bottom, null)

    /** The bottom inset now (read in composition, the reader recomposes when a live one changes). */
    val bottom: Dp get() = live?.invoke() ?: fixed

    override fun equals(other: Any?): Boolean =
        other is PhoneInsets && other.top == top && other.fixed == fixed && other.live === live

    override fun hashCode(): Int = (top.hashCode() * 31 + fixed.hashCode()) * 31 + (live?.hashCode() ?: 0)

    override fun toString(): String = "PhoneInsets(top=$top, bottom=$bottom)"

    companion object {
        /** Mobile.swift OS.android: a 44 status bar and a 24 gesture area. */
        val Lab = PhoneInsets(Phone.statusBar, Phone.gestureArea)

        /** A bottom read from snapshot state each time it is asked (the window's insets); keep the one instance. */
        fun live(top: Dp, bottom: () -> Dp): PhoneInsets = PhoneInsets(top, 0.dp, bottom)
    }
}

/**
 * Exactly `padding(start, top, end, bottom = insets.bottom + over)`, with the bottom inset read while measuring: the
 * keyboard's slide moves what is above it by a layout pass per frame and recomposes nothing (kb PHN-712).
 */
fun Modifier.paddingOverBottom(insets: PhoneInsets, over: Dp, start: Dp = 0.dp, end: Dp = 0.dp, top: Dp = 0.dp): Modifier =
    layout { measurable, constraints ->
        val s = start.roundToPx()
        val t = top.roundToPx()
        val horizontal = s + end.roundToPx()
        val vertical = t + (insets.bottom + over).roundToPx()
        val placeable = measurable.measure(constraints.offset(-horizontal, -vertical))
        layout(constraints.constrainWidth(placeable.width + horizontal), constraints.constrainHeight(placeable.height + vertical)) {
            placeable.placeRelative(s, t)
        }
    }

/**
 * Every phone screen (MobileScreens.swift MobileContent): the sky over the owner, the season's layer when [season] is
 * on (the home screen only; it fades out over 1.4 s when switched off, as a chat starts), the page below the status
 * bar, and a sheet over it. The sky's ink for what sits on it is provided to everything inside.
 */
@Composable
fun PhoneScreen(
    moment: SkyMoment,
    modifier: Modifier = Modifier,
    insets: PhoneInsets = PhoneInsets.Lab,
    season: Boolean = false,
    seasonTime: Double? = null,
    glide: Boolean = true,
    sheet: (@Composable () -> Unit)? = null,
    page: @Composable () -> Unit,
) {
    val ink = moment.ink
    val backdrop = remember(moment) { SkyBackdrop(moment.sun.elevation, moment.across, moment.moon) }
    val seasonAlpha by animateFloatAsState(if (season) 1f else 0f, Motion.seasonFade(), label = "season")
    CompositionLocalProvider(LocalSkyInk provides ink, LocalSkyBackdrop provides backdrop) {
        Box(modifier.fillMaxSize().background(Ink.black)) {
            Sky(moment, Modifier.fillMaxSize().skyBackdropHost(backdrop), glide = glide)
            if (seasonAlpha > 0f) {
                SeasonLayer(moment.season, moment.night, Modifier.fillMaxSize().alpha(seasonAlpha), seasonTime)
            }
            Box(Modifier.fillMaxSize().padding(top = insets.top)) { page() }
            sheet?.invoke()
        }
    }
}

/** Lets a row reach past its parent's padding by [by] on each side (SwiftUI's negative padding). */
fun Modifier.outset(by: Dp): Modifier = layout { measurable, constraints ->
    val extra = (by * 2).roundToPx()
    val placeable = measurable.measure(
        constraints.copy(
            minWidth = constraints.minWidth + extra,
            maxWidth = if (constraints.hasBoundedWidth) constraints.maxWidth + extra else constraints.maxWidth,
        ),
    )
    layout(placeable.width - extra, placeable.height) { placeable.place(-by.roundToPx(), 0) }
}

/**
 * Mobile.swift Device's Android hardware as the lab draws it: status bar (9:41, signal, Wi-Fi, 82 %), the camera
 * hole and the gesture bar, in the sky's ink. For previews and screenshots only: on a phone the system draws these.
 */
@Composable
fun LabAndroidChrome(modifier: Modifier = Modifier) {
    val ink = LocalSkyInk.current
    Box(modifier.fillMaxSize().clearAndSetSemantics { }) {
        Row(
            Modifier.fillMaxWidth().height(Phone.statusBar).padding(horizontal = 24.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PhoneText("9:41", 14f, FerbType.Medium, ink.top)
            Spacer(Modifier.weight(1f))
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                FerbIcon(FerbIcons.cellular, 13f, ink.top)
                FerbIcon(FerbIcons.wifi, 13f, ink.top)
                PhoneText("82%", 13f, FerbType.Medium, ink.top)
            }
        }
        Box(Modifier.align(Alignment.TopCenter).padding(top = 13.dp).size(22.dp).background(Ink.black, CircleShape))
        Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 9.dp).size(108.dp, 4.dp).background(ink.low, CircleShape))
    }
}
