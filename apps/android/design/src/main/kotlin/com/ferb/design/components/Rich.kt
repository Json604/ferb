package com.ferb.design.components

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.ParagraphStyle
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextIndent
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.ferb.design.FerbType
import com.ferb.design.Ink
import com.ferb.design.Phone
import kotlin.math.floor

// FERB's words as FERB.app draws them (kb PHN-720..): the blocks the phone library reads from FERB's Markdown
// (ferb-mobile `rich`, FERB.app's MarkdownDocument), drawn in the phone's bubble: bold, italics, strikethrough, code,
// headings, lists, quotes, code blocks, tables and links that open in the browser. Nothing here parses: the blocks come
// ready, read off the main thread and kept per entry; turning them into one styled text is remembered per bubble.

/** A run of words with one look; [link] an address to open (http, https or mailto: the library makes no other). */
@Immutable
data class RichRun(
    val text: String,
    val bold: Boolean = false,
    val italic: Boolean = false,
    val code: Boolean = false,
    val strike: Boolean = false,
    val link: String? = null,
)

/** One block of FERB's words (FERB.app's MarkdownDocument.Block). */
@Immutable
sealed interface RichBlock {
    data class Paragraph(val runs: List<RichRun>) : RichBlock
    data class Heading(val level: Int, val runs: List<RichRun>) : RichBlock
    data class Item(val marker: String, val level: Int, val runs: List<RichRun>) : RichBlock
    data class Quote(val runs: List<RichRun>) : RichBlock
    data class Code(val text: String, val language: String) : RichBlock
    data class Table(val header: List<List<RichRun>>, val rows: List<List<List<RichRun>>>) : RichBlock
    data object Divider : RichBlock
}

/** Plain words as one paragraph (FERB's own lines that are no Markdown on the Mac either). */
fun plainBlocks(text: String): List<RichBlock> = listOf(RichBlock.Paragraph(listOf(RichRun(text))))

/**
 * The chat list's visible height: an answer taller than it shows about half of it, the rest on "Show all" (FERB.app's
 * CollapsePolicy for answers, APP-719, APP-725, there with the height the owner set). Unspecified: nothing collapses.
 */
val LocalChatViewport = androidx.compose.runtime.compositionLocalOf { Dp.Unspecified }

/** The fold FERB.app's CollapsePolicy applies to a text. */
enum class Fold { NONE, ANSWER, ROW }

/** FERB.app's CollapsePolicy (CardRowState.swift:192-216): the lines a text folds to, or null when it shows whole. */
object FoldRule {
    const val ROW_LINES = 3
    const val ROW_LONGER_THAN = 8
    const val LEAST_HIDDEN = 3

    fun lines(fold: Fold, total: Int, lineHeightPx: Float, viewportPx: Float): Int? {
        if (lineHeightPx <= 0f || total <= 0) return null
        val shown = when (fold) {
            Fold.NONE -> return null
            Fold.ROW -> if (total > ROW_LONGER_THAN) ROW_LINES else return null
            Fold.ANSWER -> {
                if (viewportPx <= 0f || total * lineHeightPx <= viewportPx) return null
                maxOf(1, floor(viewportPx / 2 / lineHeightPx).toInt())
            }
        }
        return if (total - shown >= LEAST_HIDDEN) shown else null
    }
}

/** How the bubble's words look on its fill. */
@Immutable
data class RichLook(val size: Float, val ink: Color, val dim: Color, val link: Color, val codeFill: Color)

val onBlack = RichLook(16f, Ink.white, Ink.onDark, Color(0xFF9CC7FF), Ink.darkFill)
val onWhite = RichLook(15f, Ink.black, Ink.detail, Color(0xFF1F6FEB), Ink.fill)

/** The blocks as one styled text, so selection and Copy run across the whole message (APP-637). */
fun richText(blocks: List<RichBlock>, look: RichLook, tail: List<Pair<String, Boolean>> = emptyList()): AnnotatedString = buildAnnotatedString {
    val base = look.size
    fun runs(rs: List<RichRun>, bold: Boolean = false) {
        for (r in rs) {
            val style = SpanStyle(
                fontWeight = if (r.bold || bold) FontWeight.SemiBold else null,
                fontStyle = if (r.italic) FontStyle.Italic else null,
                fontFamily = if (r.code) FontFamily.Monospace else null,
                background = if (r.code) look.codeFill else Color.Unspecified,
                textDecoration = if (r.strike) TextDecoration.LineThrough else null,
            )
            val link = r.link
            if (link != null) {
                withLink(LinkAnnotation.Url(link, TextLinkStyles(SpanStyle(color = look.link, textDecoration = TextDecoration.Underline)))) {
                    withStyle(style) { append(r.text) }
                }
            } else withStyle(style) { append(r.text) }
        }
    }
    var first = true
    fun gap() {
        // FERB.app's block rhythm: a line of the text's size and a smaller one between blocks
        if (!first) withStyle(ParagraphStyle(lineHeight = 7.sp)) { append(" ") }
        first = false
    }
    for (b in blocks) {
        gap()
        when (b) {
            is RichBlock.Paragraph -> withStyle(ParagraphStyle()) { runs(b.runs) }
            is RichBlock.Heading -> withStyle(ParagraphStyle()) {
                // FERB.app: 17, 15, then 13 points over its 12: the phone's 16 scaled the same
                val size = base * when (b.level) { 1 -> 17f / 12f; 2 -> 15f / 12f; else -> 13f / 12f }
                withStyle(SpanStyle(fontSize = size.sp, fontWeight = FontWeight.SemiBold)) { runs(b.runs, bold = true) }
            }
            is RichBlock.Item -> {
                val indent = (b.level * 12).sp
                withStyle(ParagraphStyle(textIndent = TextIndent(firstLine = indent, restLine = (b.level * 12 + 20).sp))) {
                    withStyle(SpanStyle(color = look.dim)) { append(b.marker); append("  ") }
                    runs(b.runs)
                }
            }
            is RichBlock.Quote -> withStyle(ParagraphStyle(textIndent = TextIndent(12.sp, 12.sp))) {
                withStyle(SpanStyle(color = look.dim)) { runs(b.runs) }
            }
            is RichBlock.Code -> withStyle(ParagraphStyle()) {
                if (b.language.isNotEmpty()) withStyle(SpanStyle(color = look.dim, fontSize = (base * 0.75f).sp)) { append(b.language); append("\n") }
                withStyle(SpanStyle(fontFamily = FontFamily.Monospace, fontSize = (base * 11f / 12f).sp, background = look.codeFill)) { append(b.text) }
            }
            is RichBlock.Table -> withStyle(ParagraphStyle()) {
                // the phone's narrow bubble: each row on its line, its cells apart (FERB.app draws a grid)
                (listOf(b.header) + b.rows).forEachIndexed { i, row ->
                    if (i > 0) append("\n")
                    row.forEachIndexed { c, cell ->
                        if (c > 0) withStyle(SpanStyle(color = look.dim)) { append("  ·  ") }
                        runs(cell, bold = i == 0)
                    }
                }
            }
            RichBlock.Divider -> withStyle(ParagraphStyle()) { withStyle(SpanStyle(color = look.dim)) { append("────────────") } }
        }
    }
    // code's own lines under the words: "Couldn't confirm: …" and the lines in the quieter colour (FERB.app's annotations)
    for ((line, label) in tail) {
        gap()
        withStyle(ParagraphStyle()) {
            // FERB.app's annotations (MarkdownContent.swift:36-47): the note at 11.5 and code's lines at 11 under its 12,
            // grey, "Couldn't confirm:" amber and bold (kb PHN-773)
            if (label) {
                val at = line.indexOf(':') + 1
                withStyle(SpanStyle(fontSize = (base * 11.5f / 12f).sp)) {
                    withStyle(SpanStyle(color = Ink.amber, fontWeight = FontWeight.SemiBold)) { append(line.substring(0, at)) }
                    withStyle(SpanStyle(color = look.dim)) { append(line.substring(at)) }
                }
            } else withStyle(SpanStyle(color = look.dim, fontSize = (base * 11f / 12f).sp)) { append(line) }
        }
    }
}

/**
 * FERB's words in a block of text that folds as FERB.app's does: [fold] ANSWER past the chat's height (to about half of
 * it), ROW past 8 lines (to 3), with [more] ("Show all", "Show more") and "Show less". Selectable: a long press selects
 * and copies across the whole message. [key] keeps whether he opened it while the list scrolls.
 */
@Composable
fun RichWords(
    text: AnnotatedString,
    look: RichLook,
    key: String,
    fold: Fold = Fold.NONE,
    more: String = "Show all",
    less: String = "Show less",
    modifier: Modifier = Modifier,
    whenFolded: AnnotatedString? = null,
) {
    val style = FerbType.style(look.size, color = look.ink, lineSpacing = 2f)
    var open by rememberSaveable(key) { mutableStateOf(false) }
    BoxWithConstraints(modifier) {
        val viewport = LocalChatViewport.current
        val density = LocalDensity.current
        val measurer = rememberTextMeasurer(cacheSize = 4)
        val width = constraints.maxWidth
        // the lines it folds to, and where the text they hold ends
        // kept per text, width and chat height (RichCache): a message scrolling back in is not measured again (kb PHN-716)
        val folded: Pair<Int, Int>? = if (fold == Fold.NONE || width == Constraints.Infinity) null else remember(text, width, viewport) {
            RichCache.fold(text, width, viewport, fold) {
                val m = measurer.measure(text, style, constraints = Constraints(maxWidth = width))
                val lineHeight = if (m.lineCount > 0) m.size.height.toFloat() / m.lineCount else 0f
                FoldRule.lines(fold, m.lineCount, lineHeight, with(density) { if (viewport == Dp.Unspecified) 0f else viewport.toPx() })
                    ?.let { n -> n to m.getLineEnd(n - 1, visibleEnd = true) }
            }
        }
        val shown = folded?.first
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            SelectionContainer {
                // folded, only the words of its first lines are laid out: a link past them would have no line to sit
                // on (Compose 1.7's TextLinkScope fails on a link the layout cut off)
                val words = if (folded != null && !open) remember(text, folded) { text.subSequence(0, folded.second) } else text
                BasicText(words, style = style)
            }
            if (shown != null && !open && whenFolded != null) BasicText(whenFolded, style = style)
            if (shown != null) {
                PhoneText(if (open) less else more, look.size - 1f, FerbType.Semibold, look.link,
                    Modifier.heightIn(min = 32.dp).pressable({ open = !open }, label = if (open) less else more).padding(vertical = 6.dp))
            }
        }
    }
}

/** FERB's black bubble around rich words and what goes inside it (FERB.app's FerbBubble). */
@Composable
fun FerbRichBubble(
    modifier: Modifier = Modifier,
    copy: String = "",
    maxWidth: Dp = Phone.bubbleMaxFerb,
    /** On the bubble itself, before its background (the reply swipe, kb APP-1146). */
    bubble: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    androidx.compose.foundation.layout.Box(modifier.fillMaxWidth()) {
        Column(
            bubble.widthIn(max = maxWidth).background(Ink.black, RoundedFerb)
                .semantics { if (copy.isNotEmpty()) contentDescription = "FERB: $copy" }
                .padding(horizontal = 16.dp, vertical = 11.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) { content() }
    }
}

/** FERB's bubble's shape (its tail corner at the bottom left). */
val RoundedFerb = androidx.compose.foundation.shape.RoundedCornerShape(Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleCorner, Phone.bubbleTail)

/**
 * FERB's blocks drawn whole: words as one selectable text that folds as FERB.app's (see [RichWords]); a table as a grid
 * that scrolls sideways when wider than the bubble (FERB.app draws a text table; at most 12 columns and 200 rows reach
 * here, ferb-mobile `rich`). With a table the text doesn't fold (the grid has no line height to fold by).
 */
@Composable
fun RichBlocks(
    blocks: List<RichBlock>,
    look: RichLook,
    key: String,
    fold: Fold = Fold.NONE,
    more: String = "Show all",
    less: String = "Show less",
    tail: List<Pair<String, Boolean>> = emptyList(),
) {
    if (blocks.none { it is RichBlock.Table }) {
        // kept per message (RichCache): a message scrolling back in is not built again (kb PHN-716)
        val text = remember(blocks, tail, look) { RichCache.text(blocks, look, tail, 0) { richText(blocks, look, tail) } }
        val folded = remember(tail, look) { if (tail.isEmpty()) null else RichCache.text(null, look, tail, -1) { richText(emptyList(), look, tail) } }
        RichWords(text, look, key, fold, more, less, whenFolded = folded)
        return
    }
    val segments = remember(blocks) {
        val out = ArrayList<Any>()
        var run = ArrayList<RichBlock>()
        for (b in blocks) {
            if (b is RichBlock.Table) { if (run.isNotEmpty()) { out.add(run.toList()); run = ArrayList() }; out.add(b) } else run.add(b)
        }
        if (run.isNotEmpty()) out.add(run.toList())
        out
    }
    SelectionContainer {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            segments.forEachIndexed { i, seg ->
                if (seg is RichBlock.Table) TableGrid(seg, look)
                else {
                    @Suppress("UNCHECKED_CAST") val bs = seg as List<RichBlock>
                    val last = i == segments.lastIndex
                    val text = remember(bs, look, last) { RichCache.text(blocks, look, tail, i + 1) { richText(bs, look, if (last) tail else emptyList()) } }
                    BasicText(text, style = FerbType.style(look.size, color = look.ink, lineSpacing = 2f))
                }
            }
            if (segments.last() is RichBlock.Table && tail.isNotEmpty()) {
                BasicText(remember(tail, look) { richText(emptyList(), look, tail) }, style = FerbType.style(look.size, color = look.ink, lineSpacing = 2f))
            }
        }
    }
}

/**
 * FERB's words as built and measured for the chat (kb PHN-716), kept for the messages seen most recently: a message
 * that scrolls out of the list and back in finds its text and its fold here instead of building and measuring them
 * again on the frame it comes back. Keyed by the very lists the model keeps per message (the same instance while the
 * message is unchanged; a streamed word makes new ones), the look, and the part of the message. Main thread only.
 */
object RichCache {
    /** How many texts were built and folds measured (tests: a message seen again adds none). */
    var builds = 0
        private set
    var measures = 0
        private set

    private class Same(val o: Any) {
        override fun equals(other: Any?) = other is Same && other.o === o
        override fun hashCode() = System.identityHashCode(o)
    }

    private fun <K, V> lru(n: Int) = object : LinkedHashMap<K, V>(n, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<K, V>?) = size > n
    }

    private val texts = lru<List<Any?>, AnnotatedString>(KEEP)
    private val folds = lru<List<Any>, Any>(KEEP)
    private object None

    /** [blocks] by identity (the model's list; null: none), [tail] by its value (a few short lines, made anew per draw). */
    fun text(blocks: Any?, look: RichLook, tail: Any, part: Int, make: () -> AnnotatedString): AnnotatedString {
        val k = listOf(blocks?.let(::Same), tail, look, part)
        return texts[k] ?: make().also { builds++; texts[k] = it }
    }

    @Suppress("UNCHECKED_CAST")
    fun fold(text: AnnotatedString, width: Int, viewport: Dp, fold: Fold, make: () -> Pair<Int, Int>?): Pair<Int, Int>? {
        val k = listOf(Same(text), width, viewport, fold)
        val kept = folds[k]
        if (kept != null) return if (kept === None) null else kept as Pair<Int, Int>
        return make().also { measures++; folds[k] = it ?: None }
    }

    private const val KEEP = 400
}

/** A table as a grid: the header row in semibold, hairlines between rows, sideways scrolling when wider than its place. */
@Composable
private fun TableGrid(t: RichBlock.Table, look: RichLook) {
    val rows = remember(t) { listOf(t.header) + t.rows }
    val columns = t.header.size
    androidx.compose.foundation.layout.Row(Modifier.horizontalScroll(androidx.compose.foundation.rememberScrollState())) {
        for (c in 0 until columns) {
            Column(Modifier.widthIn(min = 64.dp, max = 220.dp)) {
                rows.forEachIndexed { r, row ->
                    val text = remember(row, c, look) { richText(listOf(RichBlock.Paragraph(row.getOrElse(c) { emptyList() })), look) }
                    BasicText(text, style = FerbType.style(look.size - 1f, if (r == 0) FerbType.Semibold else FerbType.Regular, look.ink),
                        modifier = Modifier.padding(end = 14.dp, top = 4.dp, bottom = 4.dp))
                }
            }
        }
    }
}
