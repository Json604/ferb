package com.ferb.design.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ferb.design.FerbIcon
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone

/** MobileScreens.swift LinkPicture.Kind. */
enum class LinkKind { DIRECT, RELAY, SEARCH }

/**
 * MobileScreens.swift LinkPicture: the phone and the Mac side by side, joined by a still line of dots (through the
 * relay's cloud when there is one), with the Mac's name and how it is reached underneath. Nothing moves.
 */
@Composable
fun LinkPicture(kind: LinkKind, macName: String = "MacBook Pro", modifier: Modifier = Modifier, status: String? = null, name: String? = null) {
    val ink = LocalSkyInk.current.low
    // [status] and [name] let the app say what the lab's three pictures don't (your own relay, a Mac that can't be reached)
    val status = status ?: when (kind) {
        LinkKind.SEARCH -> "Not paired yet"
        LinkKind.RELAY -> "Through the shared relay"
        LinkKind.DIRECT -> "Connected directly"
    }
    val name = name ?: if (kind == LinkKind.SEARCH) "Your Mac" else macName
    Column(
        modifier.fillMaxWidth().semantics(mergeDescendants = true) { contentDescription = "$name, $status" },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Disc(FerbIcons.phone, 58.dp, white = false)
            Dots(if (kind == LinkKind.RELAY) 3 else 6, ink)
            if (kind == LinkKind.RELAY) {
                Box(Modifier.size(38.dp).background(Ink.black, CircleShape), contentAlignment = Alignment.Center) {
                    FerbIcon(FerbIcons.cloud, 15f, Ink.white)
                }
                Dots(3, ink)
            }
            if (kind == LinkKind.SEARCH) {
                Box(Modifier.size(76.dp), contentAlignment = Alignment.Center) {
                    Canvas(Modifier.fillMaxSize()) {
                        val w = 1.5.dp.toPx()
                        drawCircle(ink, radius = size.minDimension / 2 - w / 2,
                            style = Stroke(w, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 5.dp.toPx()))))
                    }
                    FerbIcon(FerbIcons.laptop, 26f, ink)
                }
            } else {
                Disc(FerbIcons.laptop, 76.dp, white = true)
            }
        }
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            PhoneText(name, 24f, FerbType.Semibold, ink)
            PhoneText(status, 15.5f, FerbType.Medium, ink)
        }
    }
}

@Composable
private fun Dots(count: Int, color: Color) {
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        repeat(count) { Box(Modifier.size(4.dp).background(color, CircleShape)) }
    }
}

@Composable
private fun Disc(icon: ImageVector, size: Dp, white: Boolean) {
    Box(Modifier.size(size).background(if (white) Ink.white else Ink.black, CircleShape), contentAlignment = Alignment.Center) {
        FerbIcon(icon, size.value * 0.4f, if (white) Ink.black else Ink.white)
    }
}

/**
 * MobileScreens.swift CodesPage: the code both screens show, as three pairs of digits in grey squares (the six
 * digits of PHN-142's pairing). Fixed-width digits so the pairs line up.
 */
@Composable
fun PairingCode(code: String, modifier: Modifier = Modifier) {
    val pairs = code.filter(Char::isDigit).chunked(2)
    Row(
        modifier.fillMaxWidth().semantics(mergeDescendants = true) { contentDescription = "Code ${code.toList().joinToString(" ")}" },
        horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
    ) {
        for (pair in pairs) {
            Box(
                Modifier.sizeIn(minWidth = Phone.codeBox, minHeight = Phone.codeBox)
                    .background(Ink.fill, RoundedCornerShape(Phone.codeCorner)),
                contentAlignment = Alignment.Center,
            ) {
                BasicText(
                    pair,
                    style = FerbType.style(42f, FerbType.Semibold).copy(fontFeatureSettings = "tnum", textAlign = TextAlign.Center),
                )
            }
        }
    }
}

/** MobileScreens.swift PairPage.step: a numbered step. */
@Composable
fun PairStep(number: Int, text: String) {
    Row(Modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(26.dp).background(Ink.fill, CircleShape), contentAlignment = Alignment.Center) {
            PhoneText("$number", 13f, FerbType.Bold)
        }
        PhoneText(text, 15.5f, lineSpacing = 2f, modifier = Modifier.padding(top = 2.dp))
    }
}

/** MobileScreens.swift PairPage.action: a small labelled way out under the main button. */
@Composable
fun PairAction(icon: ImageVector, title: String, modifier: Modifier = Modifier, onClick: (() -> Unit)? = {}) {
    Column(modifier.pressable(onClick).padding(vertical = 4.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        FerbIcon(icon, 20f, Ink.black)
        PhoneText(title, 13.5f, FerbType.Medium, Ink.detail, maxLines = 1)
    }
}

/**
 * The QR scanner's frame. NOT drawn by the glass lab (its Pair page only has "Scan the code"); built from the lab's
 * own pieces (a white card of 28 corners, an Ink.fill well of 24 corners, black corner marks in the Mark's 2.5 line)
 * until the lab draws one. [preview] is where the camera's picture goes: the well clips it to its own rounded square,
 * so a picture of any size or aspect stays inside (kb PHN-618). With [live] (a camera picture under them) the corner
 * marks are white on a soft dark edge, readable on a dark room and on the Mac's white screen alike.
 */
@Composable
fun QrFrame(
    modifier: Modifier = Modifier,
    hint: String = "Point your phone at the code on your Mac.",
    live: Boolean = false,
    preview: @Composable () -> Unit = {},
) {
    Card(modifier, padding = 16.dp) {
        val well = RoundedCornerShape(Phone.codeCorner)
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f).clip(well).background(Ink.fill, well)
                .semantics { contentDescription = "Camera view for the pairing code" },
            contentAlignment = Alignment.Center,
        ) {
            preview()
            Canvas(Modifier.fillMaxSize().padding(28.dp)) {
                val len = size.minDimension * 0.18f
                val w = 4.dp.toPx()
                val r = 10.dp.toPx()
                val stroke = Stroke(w, cap = StrokeCap.Round, join = StrokeJoin.Round)
                fun corner(x: Float, y: Float, dx: Float, dy: Float) {
                    val p = Path().apply {
                        moveTo(x, y + dy * len)
                        lineTo(x, y + dy * r)
                        quadraticTo(x, y, x + dx * r, y)
                        lineTo(x + dx * len, y)
                    }
                    if (live) {
                        drawPath(p, Color.Black.copy(alpha = 0.35f), style = Stroke(w + 3.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
                        drawPath(p, Ink.white, style = stroke)
                    } else {
                        drawPath(p, Ink.black, style = stroke)
                    }
                }
                corner(0f, 0f, 1f, 1f)
                corner(size.width, 0f, -1f, 1f)
                corner(0f, size.height, 1f, -1f)
                corner(size.width, size.height, -1f, -1f)
            }
        }
        PhoneText(hint, 14f, color = Ink.detail, modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 12.dp))
    }
}
