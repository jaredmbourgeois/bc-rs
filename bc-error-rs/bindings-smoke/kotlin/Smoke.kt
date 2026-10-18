// One value across the ABI, in both directions — the Kotlin half of the same
// claim `swift/main.swift` makes. Two languages rather than one because the
// generated scaffolding differs per backend: a defect in the Kotlin lift is
// invisible to the Swift run and vice versa.

import uniffi.bc_error.*

private fun check(condition: Boolean, what: String) {
    if (!condition) {
        System.err.println("FAIL: $what")
        kotlin.system.exitProcess(1)
    }
    println("  [ok] $what")
}

fun main() {
    val text = metadataString("counter")
    check(stringifyMetadataValue(text) == "\"counter\"", "a string crosses and stringifies")
    check(metadataValueType(text) == MetadataType.STRING, "its type crosses as a closed enum")

    val number = metadataInt64(-7L)
    check(stringifyMetadataValue(number) == "-7", "a signed integer crosses")

    // A recursive value: the node stream's whole reason for existing.
    val nested = metadataArray(listOf(text, number))
    check(metadataValueType(nested) == MetadataType.ARRAY, "an array crosses as an array")
    check(
        stringifyMetadataValue(nested) == "[\"counter\", -7]",
        "its elements survive in order",
    )

    val error = ForeignError(
        ForeignErrorHeader("store", 7, "Smoke.kt > #1", listOf("reading value")),
        metadataDictionary(listOf(ForeignMetadataEntry("detail", metadataString("missing counter")))),
    )
    check(
        formatErrorMessage(error) == "reading value: store kind=7: missing counter",
        "an error built in Kotlin renders through Rust's own formatter",
    )

    println("==> kotlin bindings smoke: the ABI round-trips")
}
