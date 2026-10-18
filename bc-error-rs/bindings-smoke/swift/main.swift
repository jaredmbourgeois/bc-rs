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

// A metadata value out of Rust, matched on the foreign side. The stream is a
// flat pre-order node list, so this also proves the lift walks it correctly.
let text = metadataString(value: "counter")
check(stringifyMetadataValue(metadata: text) == "\"counter\"", "a string crosses and stringifies")
check(metadataValueType(metadata: text) == MetadataType.string, "its type crosses as a closed enum")

let number = metadataInt64(value: -7)
check(stringifyMetadataValue(metadata: number) == "-7", "a signed integer crosses")

// A recursive value: the node stream's whole reason for existing.
let nested = metadataArray(elements: [text, number])
check(metadataValueType(metadata: nested) == MetadataType.array, "an array crosses as an array")
check(stringifyMetadataValue(metadata: nested) == "[\"counter\", -7]", "its elements survive in order")

// An error assembled on the foreign side and rendered by Rust.
let header = ForeignErrorHeader(
    category: "store", kind: 7, location: "Smoke.swift > #1", context: ["reading value"])
let error = ForeignError(
    header: header,
    metadata: metadataDictionary(entries: [
        ForeignMetadataEntry(key: "detail", value: metadataString(value: "missing counter"))
    ]))
let rendered = formatErrorMessage(error: error)
check(
    rendered == "reading value: store kind=7: missing counter",
    "an error built in Swift renders through Rust's own formatter")

print("==> swift bindings smoke: the ABI round-trips")
