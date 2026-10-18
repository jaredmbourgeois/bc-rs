//! The canonical tree and its twins. The canonical tree's committed
//! `generated/telemetry/` is the golden: it must be fresh. Each twin is the same
//! tree written another way, and must generate the same bytes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::support::{Scratch, assert_answer, fixture, run_in};

/// The canonical tree written two other ways: every file in JSON, and every
/// declaration in another order and split, with windows in other units and
/// references naming their own module in full.
const TWINS: [&str; 2] = ["canonical-json", "reordered"];
const GENERATED: &str = "generated/telemetry";
/// How many files the canonical tree generates: the manifest; the PostgreSQL
/// layout's DDL, functions, views and corpus; a query file for each of its
/// three modules; for Grafana, its two dashboards and the TypeScript and Rust
/// query handles; and a C++ header and a Rust source for each module, in that
/// module's own directory.
pub(crate) const GENERATED_FILES: usize = 18;

/// Where the canonical tree's generated files are: the root's `output`, and the
/// same path under each other module's directory, where its emitting code goes.
const GENERATED_DIRECTORIES: [&str; 3] = [
    "generated/telemetry",
    "services/payments/generated/telemetry",
    "services/payments/ledger/generated/telemetry",
];

/// Every generated file under `root`, by its path from `root`.
fn generated_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for directory in GENERATED_DIRECTORIES {
        for (path, contents) in files_under(&root.join(directory)) {
            files.insert(format!("{directory}/{path}"), contents);
        }
    }
    files
}

/// Every file under `directory`, by its path from there.
fn files_under(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fn collect(directory: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("list the directory") {
            let entry = entry.expect("read the directory's entry");
            let name = entry.file_name().into_string().expect("a UTF-8 name");
            let from_the_top = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("read the entry's type").is_dir() {
                collect(&entry.path(), &from_the_top, files);
            } else {
                let contents = fs::read(entry.path()).expect("read the file");
                files.insert(from_the_top, contents);
            }
        }
    }
    let mut files = BTreeMap::new();
    collect(directory, "", &mut files);
    files
}

fn the_golden() -> BTreeMap<String, Vec<u8>> {
    let golden = generated_files(&fixture("canonical"));
    assert_eq!(
        golden.len(),
        GENERATED_FILES,
        "the golden holds {:?}",
        golden.keys().collect::<Vec<_>>()
    );
    golden
}

/// Asserts `generated` is the golden, naming the first file that is not.
#[track_caller]
fn assert_is_the_golden(generated: &BTreeMap<String, Vec<u8>>, what: &str) {
    let golden = the_golden();
    assert_eq!(
        generated.keys().collect::<Vec<_>>(),
        golden.keys().collect::<Vec<_>>(),
        "{what} generates other files than the golden holds"
    );
    for (path, contents) in &golden {
        assert!(
            generated.get(path) == Some(contents),
            "{what}'s `{path}` differs from the golden's"
        );
    }
}

#[test]
fn the_committed_golden_is_fresh() {
    let outcome = run_in(&fixture("canonical"), &["--check"]);
    assert_answer(
        &outcome,
        &format!("{GENERATED}: fresh ({GENERATED_FILES} files)\n"),
    );
}

/// With freshness above, a twin that generates the golden means equal
/// generation: JSON and TOML read the same; neither declaration order, nor the
/// split across files, nor the unit a window is written in reaches the output;
/// and a reference means the same whether or not it names its own module.
#[test]
fn json_and_a_reordered_split_generate_what_the_canonical_toml_does() {
    for name in TWINS {
        let scratch = Scratch::of_fixture(name);
        assert_answer(
            &run_in(scratch.path(), &[]),
            &format!("{GENERATED}: wrote {GENERATED_FILES}, removed 0, unchanged 0\n"),
        );
        assert_is_the_golden(&generated_files(scratch.path()), name);
    }
}

/// The golden holds no state a definition does not: it is what a first run
/// writes into an empty output directory, so it can always be regenerated from
/// the definitions alone.
#[test]
fn a_regeneration_writes_once_and_then_finds_nothing_to_write() {
    let scratch = Scratch::of_fixture("canonical");
    scratch.remove_every_generated_tree();

    let first = run_in(scratch.path(), &[]);
    assert_answer(
        &first,
        &format!("{GENERATED}: wrote {GENERATED_FILES}, removed 0, unchanged 0\n"),
    );
    assert_is_the_golden(&generated_files(scratch.path()), "a first run");

    let second = run_in(scratch.path(), &[]);
    assert_answer(
        &second,
        &format!("{GENERATED}: wrote 0, removed 0, unchanged {GENERATED_FILES}\n"),
    );
}

#[test]
fn a_tree_with_no_generate_table_is_validated_and_writes_nothing() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        "[module]\nname = \"shop\"\nschema_version = 1\n\n\
         [span.checkout]\ndescription = \"A checkout\"\n",
    );
    assert_answer(
        &run_in(scratch.path(), &[]),
        "bc-telemetry.toml: valid, 1 module; nothing to generate (the root file has no \
         `[generate]` table)\n",
    );

    scratch.write(
        "billing/bc-telemetry.toml",
        "[module]\nname = \"billing\"\nschema_version = 1\n",
    );
    assert_answer(
        &run_in(scratch.path(), &[]),
        "bc-telemetry.toml: valid, 2 modules; nothing to generate (the root file has no \
         `[generate]` table)\n",
    );
    assert!(!scratch.exists("generated"));
}

/// A root may configure the build without being a module: it then has no
/// header, and the manifest holds the modules below it and no other.
#[test]
fn a_root_that_only_configures_the_build_is_not_a_module() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", "[generate]\noutput = \"out\"\n");
    scratch.write(
        "billing/bc-telemetry.toml",
        "[module]\nname = \"billing\"\nschema_version = 1\n",
    );
    assert_answer(
        &run_in(scratch.path(), &[]),
        "out: wrote 1, removed 0, unchanged 0\n",
    );
    let manifest = scratch.read("out/manifest.json");
    assert!(
        manifest.contains("\"modules\": {\n    \"billing\": {\n      \"directory\": \"billing\","),
        "the manifest was:\n{manifest}"
    );
    assert_eq!(
        manifest.matches("\"directory\"").count(),
        1,
        "the manifest was:\n{manifest}"
    );
}
