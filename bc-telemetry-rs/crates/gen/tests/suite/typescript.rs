//! The TypeScript query handles' proof: a program builds one dashboard with
//! Grafana's Foundation SDK from the canonical tree's generated `queries.ts`;
//! the TypeScript compiler checks it against the SDK's own types, Node runs it,
//! and what it prints holds Grafana's dashboard schema.
//!
//! It needs Node and the SDK, which `scripts/fetch-typescript-sdk.sh` fetches
//! from a committed lockfile, so it is ignored by a plain run. `scripts/verify.sh`
//! runs it where both are present and says by name that it skipped it where they
//! are not; run here, it fails on anything missing rather than passing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bc_json::JsonReader;

use crate::json_schema::Schema;
use crate::support::{Scratch, fixture};

const DASHBOARD_SCHEMA: &str = include_str!("../../../dashboard/schema/dashboard.jsonschema.json");

/// The proof's own directory: its program, its compiler settings, its
/// lockfile, and the fetched SDK.
fn proof_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/typescript")
}

/// Runs a program to completion, requiring it to succeed, and returns what it
/// printed.
fn run(command: &mut Command, what: &str) -> String {
    let output = command
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("{what} could not be run: {error}"));
    assert!(
        output.status.success(),
        "{what} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

#[test]
#[ignore = "needs Node and the SDK scripts/fetch-typescript-sdk.sh fetches; scripts/verify.sh runs it"]
fn a_dashboard_built_with_the_sdk_from_the_generated_handles_holds_grafanas_schema() {
    let proof = proof_directory();
    let node_modules = proof.join("node_modules");
    assert!(
        node_modules
            .join("@grafana/grafana-foundation-sdk")
            .is_dir()
            && node_modules.join("typescript").is_dir(),
        "the SDK is not fetched: run scripts/fetch-typescript-sdk.sh"
    );

    // The program and the generated handles side by side, as one CommonJS
    // project that finds the fetched SDK where it was installed.
    let scratch = Scratch::new();
    for file in ["checkout-health.ts", "tsconfig.json"] {
        let text = std::fs::read_to_string(proof.join(file)).expect("read a proof file");
        scratch.write(file, &text);
    }
    let handles = std::fs::read_to_string(
        fixture("canonical").join("generated/telemetry/grafana/queries.ts"),
    )
    .expect("read the generated handles");
    scratch.write("queries.ts", &handles);
    scratch.write("package.json", "{\"type\": \"commonjs\"}\n");
    std::os::unix::fs::symlink(&node_modules, scratch.path().join("node_modules"))
        .expect("link the fetched SDK");

    let compiled = scratch.path().join("out");
    run(
        Command::new("node")
            .current_dir(scratch.path())
            .arg(node_modules.join("typescript/bin/tsc"))
            .args(["-p", "tsconfig.json", "--outDir"])
            .arg(&compiled),
        "the TypeScript compiler",
    );
    let printed = run(
        Command::new("node")
            .current_dir(scratch.path())
            .arg(compiled.join("checkout-health.js")),
        "the proof program",
    );

    let violations = Schema::new(DASHBOARD_SCHEMA).violations(&printed);
    assert!(violations.is_empty(), "{violations:#?}\n{printed}");

    // What it printed is the committed golden, byte for byte: the Grafana tier
    // provisions that file (grafana_in_the_loop.rs), so this is what holds the
    // file to the program, and a move of the SDK pin shows here first.
    let golden = proof.join("checkout-health.expected.json");
    let expected = std::fs::read_to_string(&golden).expect("read the golden");
    assert!(
        printed == expected,
        "the proof printed another dashboard than {} holds; it printed:\n{printed}",
        golden.display()
    );

    // And it is the dashboard the program says: its panels read the generated
    // statements, in their units.
    let dashboard = JsonReader::new(bc_json::validate_json(&printed).expect("JSON"));
    let panels = dashboard.required_object_array("panels").expect("panels");
    let kinds: Vec<String> = panels
        .iter()
        .map(|panel| panel.required_string("type").expect("a type"))
        .collect();
    assert_eq!(kinds, ["row", "timeseries", "stat", "row", "logs"]);
    let timeseries = &panels[1];
    let target = &timeseries
        .required_object_array("targets")
        .expect("targets")[0];
    assert!(
        handles.contains(&bc_json_string(
            &target.required_string("rawSql").expect("raw SQL")
        )),
        "the panel reads SQL the handles do not hold"
    );
    assert_eq!(
        target.required_string("format").expect("format"),
        "time_series"
    );
    assert!(target.required_boolean("rawQuery").expect("raw query"));
}

/// `text` spelled as the handles file spells a string: a JSON string literal.
fn bc_json_string(text: &str) -> String {
    let mut json = bc_json::JsonWriter::new();
    json.bare_array(|json| json.array_element(text));
    let array = json.finish().expect("a string array");
    array[1..array.len() - 1].to_string()
}
