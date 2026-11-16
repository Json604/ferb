package com.ferb.design

/** A small JSON reader for the checked-in lab numbers (no JSON library on the unit-test path). */
object MiniJson {
    fun parse(text: String): Any? = Reader(text).run { val v = value(); ws(); require(i == s.length) { "trailing data at $i" }; v }

    fun resource(name: String): Any? =
        parse(requireNotNull(MiniJson::class.java.classLoader!!.getResource(name)) { "missing $name" }.readText())

    private class Reader(val s: String) {
        var i = 0
        fun ws() { while (i < s.length && s[i].isWhitespace()) i++ }
        fun value(): Any? {
            ws()
            return when (val c = s[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> { i += 4; true }
                'f' -> { i += 5; false }
                'n' -> { i += 4; null }
                else -> if (c == '-' || c.isDigit()) num() else error("bad json at $i: $c")
            }
        }
        fun obj(): Map<String, Any?> {
            val m = LinkedHashMap<String, Any?>(); i++; ws()
            if (s[i] == '}') { i++; return m }
            while (true) { ws(); val k = str(); ws(); i++; m[k] = value(); ws(); if (s[i++] == '}') return m }
        }
        fun arr(): List<Any?> {
            val l = ArrayList<Any?>(); i++; ws()
            if (s[i] == ']') { i++; return l }
            while (true) { l += value(); ws(); if (s[i++] == ']') return l }
        }
        fun str(): String {
            val b = StringBuilder(); i++
            while (s[i] != '"') {
                if (s[i] == '\\') {
                    i++
                    when (val e = s[i]) {
                        'n' -> b.append('\n'); 't' -> b.append('\t'); 'r' -> b.append('\r'); 'b' -> b.append('\b'); 'f' -> b.append('\u000C')
                        'u' -> { b.append(s.substring(i + 1, i + 5).toInt(16).toChar()); i += 4 }
                        else -> b.append(e)
                    }
                } else b.append(s[i])
                i++
            }
            i++
            return b.toString()
        }
        fun num(): Double {
            val start = i
            while (i < s.length && (s[i].isDigit() || s[i] in "+-.eE")) i++
            return s.substring(start, i).toDouble()
        }
    }
}

@Suppress("UNCHECKED_CAST")
fun Any?.obj() = this as Map<String, Any?>
@Suppress("UNCHECKED_CAST")
fun Any?.list() = this as List<Any?>
fun Any?.num() = (this as Number).toDouble()
fun Any?.nums() = list().map { it.num() }
