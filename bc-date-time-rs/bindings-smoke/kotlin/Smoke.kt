// One value across the ABI, in both directions — the Kotlin half of the same
// claim `swift/main.swift` makes. Two languages rather than one because the
// generated scaffolding differs per backend: a defect in the Kotlin lift is
// invisible to the Swift run and vice versa.

import uniffi.bc_date_time.*

private fun check(condition: Boolean, what: String) {
    if (!condition) {
        System.err.println("FAIL: $what")
        kotlin.system.exitProcess(1)
    }
    println("  [ok] $what")
}

fun main() {
    val parsed = parseIsoTimestamp("2026-08-30T12:34:56.000Z")
        ?: run {
            System.err.println("FAIL: a valid ISO timestamp did not parse")
            kotlin.system.exitProcess(1)
        }
    check(
        formatIsoTimestamp(parsed) == "2026-08-30 12:34:56 UTC",
        "a timestamp parsed from the ISO form renders in the space-separated one",
    )
    check(formatIsoDate(parsed) == "2026-08-30", "the same instant renders as a date")

    check(parseIsoDate("not a date") == null, "an unparseable date crosses as null")

    val components = epochToComponents(parsed)
    check(components.year == 2026, "a record's fields survive the crossing")
    check(components.month == Month.AUGUST, "a discriminant-pinned enum crosses closed")
    check(componentsToEpoch(components) == parsed, "components round-trip back")

    check(isLeapYear(2024) && !isLeapYear(2026), "leap years agree")
    check(daysInMonth(2024, Month.FEBRUARY).toInt() == 29, "February 2024 has 29 days")

    println("==> kotlin bindings smoke: the ABI round-trips")
}
