//! Finding the tree's modules: every directory under the root that holds a
//! definition file is one, and the walk skips exactly the directories that
//! would otherwise read as a second copy of the tree.

use crate::support::{Scratch, assert_answer, assert_findings, run_in};

/// A root that is the module `shop` and generates into `generated/telemetry`.
const ROOT: &str = "[module]\nname = \"shop\"\nschema_version = 1\n\n\
                    [generate]\noutput = \"generated/telemetry\"\n";

fn module(name: &str) -> String {
    format!("[module]\nname = \"{name}\"\nschema_version = 1\n")
}

const WROTE_THE_MANIFEST: &str = "generated/telemetry: wrote 1, removed 0, unchanged 0\n";

/// The modules a run found, read back from the manifest it wrote.
fn modules_in_the_manifest(scratch: &Scratch) -> Vec<String> {
    let manifest = scratch.read("generated/telemetry/manifest.json");
    let mut modules = Vec::new();
    // A module's entry is the one object at this depth that opens with its
    // directory.
    let mut lines = manifest.lines().peekable();
    while let Some(line) = lines.next() {
        let opens_a_module = lines
            .peek()
            .is_some_and(|next| next.starts_with("      \"directory\": "));
        if opens_a_module {
            let name = line
                .trim()
                .trim_end_matches("\": {")
                .trim_start_matches('"');
            modules.push(name.to_string());
        }
    }
    modules
}

#[test]
fn a_definition_file_in_any_directory_below_the_root_is_a_module() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("services/billing/bc-telemetry.toml", &module("billing"));
    scratch.write(
        "services/billing/ledger/bc-telemetry.json",
        "{\"module\": {\"name\": \"ledger\", \"schema_version\": 1}}\n",
    );
    scratch.write("services/README.md", "no definition here\n");
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(
        modules_in_the_manifest(&scratch),
        ["billing", "ledger", "shop"]
    );
    let manifest = scratch.read("generated/telemetry/manifest.json");
    assert!(manifest.contains("\"directory\": \"services/billing/ledger\""));
    assert!(manifest.contains("\"directory\": \"services/billing\""));
}

/// A hidden directory, a build tool's output tree and the generator's own
/// output directories each hold what would read as a duplicate of a module.
#[test]
fn a_hidden_directory_a_tagged_one_and_the_generators_own_are_not_entered() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/bc-telemetry.toml", &module("billing"));
    // Each of these would be a second `billing`.
    scratch.write(".cache/billing/bc-telemetry.toml", &module("billing"));
    scratch.write(
        "target/package/billing/bc-telemetry.toml",
        &module("billing"),
    );
    scratch.write(
        "target/CACHEDIR.TAG",
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(modules_in_the_manifest(&scratch), ["billing", "shop"]);

    // Without the tag the copy is read, and is the duplicate it looks like.
    scratch.remove("target/CACHEDIR.TAG");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "target/package/billing/bc-telemetry.toml:2:8: error[duplicate-module]: module \
             `billing` is declared twice; first at billing/bc-telemetry.toml:2:8 — a module's \
             name is unique across the tree",
        ],
    );
}

/// The tag marks a build tool's output wherever it is found below the root; on
/// the root itself it marks nothing the run was not told to read.
#[test]
fn the_root_directory_is_read_whatever_marks_it() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write(
        "CACHEDIR.TAG",
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    scratch.write("billing/bc-telemetry.toml", &module("billing"));
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(modules_in_the_manifest(&scratch), ["billing", "shop"]);
}

/// A file under a directory the generator owns is never a definition, whatever
/// its name: it is a file no run generated, and is refused as that.
#[test]
fn a_definition_named_file_in_a_directory_the_generator_owns_is_not_a_module() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/bc-telemetry.toml", &module("billing"));
    scratch.write(
        "generated/telemetry/stray/bc-telemetry.toml",
        &module("stray"),
    );
    scratch.write(
        "billing/generated/telemetry/bc-telemetry.toml",
        &module("stray"),
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "error[output-unexpected]: `billing/generated/telemetry/bc-telemetry.toml` is in a \
             directory the generator owns, and no run generated it; nothing was written — move \
             it out, or choose another `output`",
            "error[output-unexpected]: `generated/telemetry/stray/bc-telemetry.toml` is in a \
             directory the generator owns, and no run generated it; nothing was written — move \
             it out, or choose another `output`",
        ],
    );
}

/// Only the whole `output` path is the generator's. A module may sit in a
/// directory that shares that path's first segment.
#[test]
fn a_module_in_a_directory_named_like_the_outputs_first_segment_is_found() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/generated/bc-telemetry.toml", &module("billing"));
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(modules_in_the_manifest(&scratch), ["billing", "shop"]);
    assert!(
        scratch
            .read("generated/telemetry/manifest.json")
            .contains("\"directory\": \"billing/generated\"")
    );
}

#[test]
fn a_directory_holding_both_syntaxes_is_refused_not_picked_from() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/bc-telemetry.toml", &module("billing"));
    scratch.write(
        "billing/bc-telemetry.json",
        "{\"module\": {\"name\": \"billing\", \"schema_version\": 1}}\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "error[ambiguous-definition]: both `bc-telemetry.toml` and `bc-telemetry.json` are in \
             `billing`; a directory holds its definition in one syntax",
        ],
    );
}

/// A file system that ignores case would read this file and one that does not
/// would pass it over, so it is refused on both. The name is the one this test
/// writes, on either.
#[test]
fn a_file_named_like_a_definition_but_for_its_case_is_refused() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/BC-Telemetry.toml", &module("billing"));
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "error[misnamed-definition]: `billing/BC-Telemetry.toml` differs from \
             `bc-telemetry.toml` by case alone; a definition file carries that exact name, so \
             every file system reads the same tree",
        ],
    );
}

#[test]
fn a_file_that_is_neither_a_module_nor_the_build_root_is_refused() {
    // A root with nothing in it.
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", "");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:1: error[missing-field]: the definition file `bc-telemetry.toml` \
             is missing its required `module`",
        ],
    );

    // A root that configures the build and also declares, without saying which
    // module the declaration belongs to.
    scratch.write(
        "bc-telemetry.toml",
        "[generate]\noutput = \"out\"\n\n[span.checkout]\ndescription = \"A checkout\"\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:1: error[missing-field]: the definition file `bc-telemetry.toml` \
             is missing its required `module`",
        ],
    );

    // A file below the root is a module and nothing else.
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write(
        "billing/bc-telemetry.toml",
        "[span.charge]\ndescription = \"A charge\"\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "billing/bc-telemetry.toml:1:1: error[missing-field]: the definition file \
             `billing/bc-telemetry.toml` is missing its required `module`",
        ],
    );
}

#[test]
fn what_only_the_root_may_say_is_refused_in_a_modules_file() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write(
        "billing/bc-telemetry.toml",
        &format!(
            "{}\n[generate]\noutput = \"out\"\n\n[retired]\nmodules = []\n",
            module("billing")
        ),
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "billing/bc-telemetry.toml:5:2: error[root-only]: `generate` belongs to the root \
             definition file only; a module's file carries `[module]`, `include` and declarations",
            "billing/bc-telemetry.toml:8:2: error[root-only]: `retired` belongs to the root \
             definition file only; a module's file carries `[module]`, `include` and declarations",
        ],
    );
}

/// Modules are found in name order and recorded in name order, whatever order
/// the directories were made in.
#[test]
fn findings_follow_the_walk_and_the_walk_follows_names() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    for name in ["zeta", "alpha", "middle"] {
        scratch.write(
            &format!("{name}/bc-telemetry.toml"),
            &format!("{}extra = 1\n", module(name)),
        );
    }
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "alpha/bc-telemetry.toml:4:1: error[unknown-field]: unknown field `extra` on `[module]`",
            "middle/bc-telemetry.toml:4:1: error[unknown-field]: unknown field `extra` on \
             `[module]`",
            "zeta/bc-telemetry.toml:4:1: error[unknown-field]: unknown field `extra` on `[module]`",
        ],
    );
}

/// A link to a directory is not followed: this one leads back to the root, so
/// following it would never end, and would find every module again on the way.
#[cfg(unix)]
#[test]
fn a_symbolic_link_to_a_directory_is_not_followed() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write("billing/bc-telemetry.toml", &module("billing"));
    std::os::unix::fs::symlink(
        scratch.path(),
        scratch.path().join("billing/back-to-the-root"),
    )
    .expect("link back to the root");
    std::os::unix::fs::symlink(
        scratch.path().join("billing"),
        scratch.path().join("billing-again"),
    )
    .expect("link to the module's directory");
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(modules_in_the_manifest(&scratch), ["billing", "shop"]);
}

/// A link to a *file* is read as the file it leads to: a module's definition
/// may be kept elsewhere and linked into the directory it describes.
#[cfg(unix)]
#[test]
fn a_definition_file_that_is_a_symbolic_link_is_read() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", ROOT);
    scratch.write(".definitions/billing.toml", &module("billing"));
    std::fs::create_dir_all(scratch.path().join("billing")).expect("create billing/");
    std::os::unix::fs::symlink(
        scratch.path().join(".definitions/billing.toml"),
        scratch.path().join("billing/bc-telemetry.toml"),
    )
    .expect("link the definition into its module's directory");
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert_eq!(modules_in_the_manifest(&scratch), ["billing", "shop"]);
}
