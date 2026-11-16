package com.ferb.app.model

import androidx.annotation.StringRes
import com.ferb.app.R
import com.ferb.mobile.SettingValue

/** How a setting is shown: a switch, a number of days, or a list changed only on the Mac. */
enum class SettingKind { SWITCH, DAYS, LIST }

/** One of FERB's settings the phone shows, with FERB.app's own title (SettingsPages.swift). */
data class SettingSpec(val key: String, @StringRes val title: Int, val kind: SettingKind)

/** A setting's value as read: a switch, days, or the list's entries. */
data class SettingRow(val spec: SettingSpec, val on: Boolean = false, val days: Int? = null, val items: List<String> = emptyList())

/**
 * FERB's settings on the phone (PHN-404, PHN-112): what FERB.app's Privacy page shows, in its order. The switch and
 * the day counts change from here, each change signed after one fingerprint (PHN-133); the two lists are shown and
 * changed in FERB on the Mac (their editor is the Mac's). browsing.owner_profile is not shown: it names a browser
 * profile only the Mac can pick.
 */
object MacSettings {
    val SPECS = listOf(
        SettingSpec("sense.window_titles", R.string.set_window_titles, SettingKind.SWITCH),
        SettingSpec("privacy.exclude_apps", R.string.set_exclude_apps, SettingKind.LIST),
        SettingSpec("privacy.exclude_domains", R.string.set_exclude_domains, SettingKind.LIST),
        SettingSpec("sense.trail_retention_days", R.string.set_trail_days, SettingKind.DAYS),
        SettingSpec("retention.trace_days", R.string.set_trace_days, SettingKind.DAYS),
        SettingSpec("retention.decision_days", R.string.set_decision_days, SettingKind.DAYS),
        SettingSpec("retention.evidence_days", R.string.set_evidence_days, SettingKind.DAYS),
        SettingSpec("retention.recovery_days", R.string.set_recovery_days, SettingKind.DAYS),
        SettingSpec("retention.resource_days", R.string.set_resource_days, SettingKind.DAYS),
        SettingSpec("retention.operational_days", R.string.set_operational_days, SettingKind.DAYS),
    )

    /** The rows the Mac gave a value for, in [SPECS]' order. */
    fun rows(values: List<SettingValue>): List<SettingRow> {
        val byKey = values.associate { it.key to it.valueJson }
        return SPECS.mapNotNull { spec -> byKey[spec.key]?.let { row(spec, it) } }
    }

    fun row(spec: SettingSpec, json: String): SettingRow {
        val v = runCatching { Json.parse(json) }.getOrNull()
        return when (spec.kind) {
            SettingKind.SWITCH -> SettingRow(spec, on = v == true)
            SettingKind.DAYS -> SettingRow(spec, days = (v as? Double)?.toInt())
            SettingKind.LIST -> SettingRow(spec, items = (v as? List<*>)?.mapNotNull { it as? String }.orEmpty())
        }
    }

    fun spec(key: String): SettingSpec? = SPECS.firstOrNull { it.key == key }
}
