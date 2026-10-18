// One value across the ABI, in both directions.
//
// Every other binding test in this family exercises the Rust side of the
// boundary only. This one is compiled from the *generated* Swift and linked
// against the real cdylib, so a scaffolding-level regression — the class no
// Rust-side test can reach — fails here instead of reaching a consumer.

import Foundation

func check(_ condition: Bool, _ what: String) {
    if !condition {
        FileHandle.standardError.write("FAIL: \(what)\n".data(using: .utf8)!)
        exit(1)
    }
    print("  [ok] \(what)")
}

// A timestamp parsed by Rust, formatted by Rust, compared in Swift: the
// newtype-over-builtin crossing, in both directions.
guard let parsed = parseIsoTimestamp(text: "2026-08-30T12:34:56.000Z") else {
    FileHandle.standardError.write("FAIL: a valid ISO timestamp did not parse\n".data(using: .utf8)!)
    exit(1)
}
check(
    formatIsoTimestamp(epoch: parsed) == "2026-08-30 12:34:56 UTC",
    "a timestamp parsed from the ISO form renders in the space-separated one")
check(formatIsoDate(epoch: parsed) == "2026-08-30", "the same instant renders as a date")

// A refusal crosses as `nil`, not as a zero value.
check(parseIsoDate(text: "not a date") == nil, "an unparseable date crosses as nil")

// A record crossing field by field, and a fieldless enum with written-out
// discriminants crossing as a closed Swift enum.
let components = epochToComponents(epoch: parsed)
check(components.year == 2026, "a record's fields survive the crossing")
check(components.month == Month.august, "a discriminant-pinned enum crosses closed")
check(componentsToEpoch(components: components) == parsed, "components round-trip back")

// Pure calendar arithmetic, computed in Rust.
check(isLeapYear(year: 2024) && !isLeapYear(year: 2026), "leap years agree")
check(daysInMonth(year: 2024, month: Month.february) == 29, "February 2024 has 29 days")

print("==> swift bindings smoke: the ABI round-trips")
