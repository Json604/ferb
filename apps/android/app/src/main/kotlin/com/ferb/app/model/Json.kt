package com.ferb.app.model

/**
 * A small JSON reader for the two texts the app reads itself: a question's body (`QuestionItem.bodyJson`, no fixed
 * shape) and the home screen's words (`assets/corpus.json`). Android's org.json is a stub on the JVM tests' classpath,
 * so the app carries this instead of a library. Values: Map, List, String, Double, Boolean, null.
 */
object Json {
    fun parse(text: String): Any? = Reader(text).run { val v = value(); space(); require(i == s.length) { "trailing text" }; v }

    private class Reader(val s: String) {
        var i = 0

        fun space() { while (i < s.length && s[i].isWhitespace()) i++ }

        fun value(): Any? {
            space()
            require(i < s.length) { "unexpected end" }
            return when (val c = s[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> word("true", true)
                'f' -> word("false", false)
                'n' -> word("null", null)
                else -> if (c == '-' || c.isDigit()) num() else error("unexpected '$c' at $i")
            }
        }

        private fun word(w: String, v: Any?): Any? {
            require(s.startsWith(w, i)) { "bad literal at $i" }
            i += w.length
            return v
        }

        private fun num(): Double {
            val start = i
            while (i < s.length && (s[i].isDigit() || s[i] in "+-.eE")) i++
            return s.substring(start, i).toDouble()
        }

        private fun str(): String {
            i++ // "
            val out = StringBuilder()
            while (true) {
                require(i < s.length) { "unterminated string" }
                when (val c = s[i++]) {
                    '"' -> return out.toString()
                    '\\' -> {
                        when (val e = s[i++]) {
                            'n' -> out.append('\n'); 't' -> out.append('\t'); 'r' -> out.append('\r')
                            'b' -> out.append('\b'); 'f' -> out.append('\u000C')
                            'u' -> { out.append(s.substring(i, i + 4).toInt(16).toChar()); i += 4 }
                            else -> out.append(e)
                        }
                    }
                    else -> out.append(c)
                }
            }
        }

        private fun arr(): List<Any?> {
            i++
            val out = ArrayList<Any?>()
            space()
            if (s[i] == ']') { i++; return out }
            while (true) {
                out.add(value())
                space()
                when (s[i++]) { ']' -> return out; ',' -> Unit; else -> error("expected , or ] at $i") }
            }
        }

        private fun obj(): Map<String, Any?> {
            i++
            val out = LinkedHashMap<String, Any?>()
            space()
            if (s[i] == '}') { i++; return out }
            while (true) {
                space()
                val k = str()
                space()
                require(s[i++] == ':') { "expected : at $i" }
                out[k] = value()
                space()
                when (s[i++]) { '}' -> return out; ',' -> Unit; else -> error("expected , or } at $i") }
            }
        }
    }
}

/** Typed reads of a parsed JSON object. */
@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.str(key: String): String? = (this[key] as? String)?.takeIf { it.isNotEmpty() }
fun Map<String, Any?>.bool(key: String): Boolean = this[key] as? Boolean ?: false
@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.strings(key: String): List<String> = (this[key] as? List<Any?>)?.mapNotNull { it as? String } ?: emptyList()
@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.obj(key: String): Map<String, Any?>? = this[key] as? Map<String, Any?>
