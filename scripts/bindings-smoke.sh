#!/usr/bin/env bash
#
# Generates a bound crate's foreign bindings in one language and runs them, for the
# bc-rs member named as its first argument — every member that carries a
# `bindings-smoke/` shim calls it from its own `scripts/verify.sh`, once per language:
#
#   scripts/bindings-smoke.sh <member-directory> swift|kotlin
#
# One copy for the workspace, where there was one per member that carried a shim.
#
# Exit codes: 0 the bindings were built and run; 1 they failed; 3 this host has no
# toolchain for the language, so nothing ran; 64 a malformed command line.
#
# WHY THIS EXISTS. Every other binding test in this family exercises the Rust
# side of the boundary only: `tests/uniffi_bindings.rs` round-trips the mirror
# types in-process and never generates a line of Swift or Kotlin. That leaves a
# whole class invisible — a scaffolding-level regression, of the kind
# `#[track_caller]` produced, is not reachable from any Rust-side test and would
# first be seen by a consumer. This step is the smallest thing that closes it:
# one value across the real ABI, in both directions, in both languages.
#
# TWO LANGUAGES, not one, because the generated scaffolding differs per backend.
# A defect in the Kotlin lift is invisible to the Swift run and vice versa.
#
# TIERING, and why absence alone does not excuse it. Swift and Kotlin are not
# build dependencies of any member, and a fresh clone must still be able to run
# `scripts/verify.sh`. But "the toolchain is missing, so this passed" is the
# fail-open the step exists to prevent. So a missing toolchain is exit 3, which the
# member's pipeline records as the skipped tier `bindings-swift` or
# `bindings-kotlin`; its verdict is INCOMPLETE unless the caller accepted that tier
# by name, with its reason (scripts/lib/member-pipeline.sh). CI names the one it
# accepts in its own workflow, beside its reason. A present toolchain that fails is
# a failure.

set -euo pipefail

if [[ $# -ne 2 || ( "$2" != swift && "$2" != kotlin ) ]]; then
    echo 'usage: scripts/bindings-smoke.sh <member-directory> swift|kotlin' >&2
    exit 64
fi
repository_root="$(cd "$1" && pwd)"
language="$2"
shim_directory="$repository_root/bindings-smoke"

if [[ ! -d "$shim_directory" ]]; then
    echo "error: $shim_directory does not exist, so there is no shim to run." >&2
    echo '       Only a member that carries a shim calls this script.' >&2
    exit 1
fi

cd "$shim_directory"

step() {
    printf '\n\033[1m--> %s\033[0m\n' "$1"
}

# Homebrew's OpenJDK is keg-only, so `java` on a default PATH is the macOS stub
# that reports no runtime at all. Look there before deciding Java is absent.
if [[ -d /opt/homebrew/opt/openjdk/bin ]]; then
    PATH="/opt/homebrew/opt/openjdk/bin:$PATH"
fi

# --- The tier decision, taken before anything is built ---------------------
#
# Deciding first is what keeps a host without the toolchain from spending a
# minute compiling a shim and generating bindings it will then not run.

case "$language" in
    swift)
        if ! command -v swiftc >/dev/null 2>&1; then
            echo 'note: swiftc is not installed, so the Swift bindings were not generated or run.'
            exit 3
        fi
        ;;
    kotlin)
        if ! command -v kotlinc >/dev/null 2>&1 || ! java -version >/dev/null 2>&1; then
            echo 'note: kotlinc or a Java runtime is not installed, so the Kotlin bindings were not'
            echo '      generated or run. Install them with: brew install kotlin'
            exit 3
        fi
        ;;
esac

# --- What to build ---------------------------------------------------------

# The shim's `[lib] name` is what `-l` and JNA take. Read from the manifest
# rather than restated here: this script serves every member with a shim, so a
# name written down would have to be a per-member config file whose only content
# is a fact cargo already knows.
shim_link_name="$(python3 -c '
import tomllib

with open("Cargo.toml", "rb") as manifest:
    package = tomllib.load(manifest)["package"]["name"]
# Cargo derives the library name from the package name this way; the manifest
# spells it out too, but the derivation is the rule.
print(package.replace("-", "_"))
')"

case "$(uname -s)" in
    Darwin) shim_library="lib${shim_link_name}.dylib" ;;
    *) shim_library="lib${shim_link_name}.so" ;;
esac

# JNA is the Kotlin bindings' only runtime dependency. Pinned by version AND by
# digest: an unpinned jar fetched over the network is a supply-chain edge that
# nothing in this repository reviewed.
jna_version='5.14.0'
jna_sha256='34ed1e1f27fa896bca50dbc4e99cf3732967cec387a7a0d5e3486c09673fe8c6'
jna_url="https://repo1.maven.org/maven2/net/java/dev/jna/jna/${jna_version}/jna-${jna_version}.jar"

build_directory="$shim_directory/target/bindings-smoke"
rm -rf "$build_directory"
mkdir -p "$build_directory"

# The generator runs on its own toolchain: `uniffi`'s `cli` feature pulls
# `cargo_metadata`, which requires a rustc newer than this family's MSRV pin.
# That pin governs the library; it does not govern a build tool, and
# `rust-toolchain.toml` would otherwise apply it to both. It is the workspace's
# pinned stable — the second leg of `.forgejo/workflows/ci.yml`'s `rust:` matrix —
# and never the moving channel, which rustup would fetch on the day: a release that moved under
# the tree would redden a push for a change it never made. It moves with that
# matrix, in the commit carrying the new release's fixes.
generator_toolchain='+1.99.0'

step "Building the cdylib shim"
cargo "$generator_toolchain" build --quiet

step "Generating the $language bindings"
cargo "$generator_toolchain" run --quiet --bin uniffi-bindgen -- \
    generate --library "target/debug/$shim_library" \
    --language "$language" --no-format \
    --out-dir "$build_directory/$language"

if [[ "$language" == swift ]]; then
    step "Swift"
    swift_directory="$build_directory/swift"
    cp swift/main.swift "$swift_directory/"
    # EVERY generated source, not just this crate's. A bound crate whose
    # boundary names a type from a sibling's binding — as bc-log-rs's
    # `ForeignLogState` carried bc-error-rs's `ForeignMetadata`, before it was
    # archived — generates that sibling's bindings too, and compiling only its
    # own leaves the converters undefined. Globbing is what keeps this correct
    # as the boundary grows.
    #
    # The generator names each modulemap after its module rather than
    # `module.modulemap`, the only name clang finds from `-I` alone, so each is
    # named explicitly. The module cache is per-run, so a stale one cannot mask
    # a change.
    swift_module_map_flags=()
    for module_map in "$swift_directory"/*FFI.modulemap; do
        swift_module_map_flags+=(-Xcc "-fmodule-map-file=$module_map")
    done
    swift_sources=()
    for source in "$swift_directory"/*.swift; do
        swift_sources+=("$(basename "$source")")
    done
    # The smoke finds the shim through an rpath linked into it, not a loader
    # variable. On Linux the shim's `.so` is recorded by its bare name, so the
    # loader needs a directory to search, and `DYLD_LIBRARY_PATH` — which this
    # once passed — is read by macOS's loader alone: Linux CI exited 127 at
    # this step (bc-error-rs task 1358). On macOS the shim's install name is
    # already its absolute path, so the rpath goes unused there.
    ( cd "$swift_directory" \
        && swiftc -o smoke "${swift_sources[@]}" \
            -I "$swift_directory" \
            "${swift_module_map_flags[@]}" \
            -L "$shim_directory/target/debug" \
            -l"$shim_link_name" \
            -Xlinker -rpath -Xlinker "$shim_directory/target/debug" \
            -module-cache-path "$build_directory/swift-module-cache" )
    "$swift_directory/smoke"
else
    step "Kotlin"
    kotlin_directory="$build_directory/kotlin"
    jna_cache="$shim_directory/target/jna-${jna_version}.jar"
    if [[ ! -f "$jna_cache" ]]; then
        echo "fetching JNA ${jna_version}"
        curl -sSLf -o "$jna_cache.partial" "$jna_url"
        mv "$jna_cache.partial" "$jna_cache"
    fi
    observed_digest="$(shasum -a 256 "$jna_cache" | cut -d' ' -f1)"
    if [[ "$observed_digest" != "$jna_sha256" ]]; then
        echo "error: the JNA jar at $jna_cache does not match its pinned digest." >&2
        echo "       expected $jna_sha256" >&2
        echo "       observed $observed_digest" >&2
        exit 1
    fi

    cp kotlin/Smoke.kt "$kotlin_directory/"
    # Every generated source, for the reason the Swift block above states.
    kotlin_sources=()
    for source in "$kotlin_directory"/uniffi/*/*.kt; do
        kotlin_sources+=("${source#"$kotlin_directory"/}")
    done
    # The compiler's own status decides; its warnings are filtered from what is
    # shown, never from what is judged. The output goes through a file so the
    # filter cannot stand in for kotlinc's exit status.
    kotlin_log="$build_directory/kotlinc.log"
    if ! ( cd "$kotlin_directory" \
        && kotlinc -nowarn -classpath "$jna_cache" \
            "${kotlin_sources[@]}" Smoke.kt \
            -include-runtime -d smoke.jar > "$kotlin_log" 2>&1 ); then
        cat "$kotlin_log" >&2
        echo 'error: kotlinc could not compile the generated Kotlin bindings.' >&2
        exit 1
    fi
    grep -v '^warning:' "$kotlin_log" || true
    java --enable-native-access=ALL-UNNAMED \
        -cp "$kotlin_directory/smoke.jar:$jna_cache" \
        -Djna.library.path="$shim_directory/target/debug" \
        SmokeKt
fi

echo
printf '\033[1;32m==> bindings smoke: the generated %s bindings were built and run\033[0m\n' "$language"
