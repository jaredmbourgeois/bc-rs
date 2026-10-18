//! A module spread over files: its own definition file is the one authority
//! for its name and version, an included file repeats both, and the layout may
//! not say two things at once — nor may one module reach into another's
//! directory.

use crate::support::{Scratch, assert_answer, assert_findings, run_in};

const HEADER: &str = "[module]\nname = \"shop\"\nschema_version = 1\n";

fn root_including(patterns: &str) -> String {
    format!("include = [{patterns}]\n\n{HEADER}")
}

#[test]
fn an_included_file_that_disagrees_with_its_modules_header_is_refused() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", &root_including("\"parts/*.toml\""));
    scratch.write(
        "parts/a.toml",
        "[module]\nname = \"shop\"\nschema_version = 2\n",
    );
    scratch.write(
        "parts/b.toml",
        "[module]\nname = \"store\"\nschema_version = 1\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "parts/a.toml:3:18: error[version-mismatch]: `schema_version` is 2 here, but the \
             module declares 1; an included file repeats its module's version and never sets its \
             own",
            "parts/b.toml:2:8: error[module-mismatch]: this file belongs to module `store`, but \
             the file that includes it is module `shop`",
        ],
    );
}

#[test]
fn an_item_declared_in_two_files_is_refused_naming_the_first() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &format!(
            "{}\n[span.checkout]\ndescription = \"A checkout\"\n",
            root_including("\"more.toml\"")
        ),
    );
    scratch.write(
        "more.toml",
        &format!("{HEADER}\n[span.checkout]\ndescription = \"Another checkout\"\n"),
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "more.toml:5:7: error[duplicate-item]: span `checkout` is declared twice; first at \
           bc-telemetry.toml:7:7",
        ],
    );
}

/// Two modules are two namespaces: the name one declares is free in the other.
#[test]
fn one_item_name_in_two_modules_is_two_items() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &format!("{HEADER}\n[span.checkout]\ndescription = \"A checkout\"\n"),
    );
    scratch.write(
        "billing/bc-telemetry.toml",
        "[module]\nname = \"billing\"\nschema_version = 7\n\n\
         [span.checkout]\ndescription = \"Billing's own checkout\"\n",
    );
    assert_answer(
        &run_in(scratch.path(), &[]),
        "bc-telemetry.toml: valid, 2 modules; nothing to generate (the root file has no \
         `[generate]` table)\n",
    );
}

#[test]
fn an_included_file_carries_a_header_and_declarations_and_nothing_else() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", &root_including("\"more.toml\""));
    scratch.write(
        "more.toml",
        &format!(
            "include = [\"other.toml\"]\n\n{HEADER}\n[generate]\noutput = \"out\"\n\n\
             [retired]\nmodules = []\n"
        ),
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "more.toml:1:1: error[invalid-include]: `include` belongs to a module's own \
             definition file; an included file carries `[module]` and declarations",
            "more.toml:7:2: error[root-only]: `generate` belongs to the root definition file \
             only; an included file carries `[module]` and declarations",
            "more.toml:10:2: error[root-only]: `retired` belongs to the root definition file \
             only; an included file carries `[module]` and declarations",
        ],
    );
}

#[test]
fn an_include_that_reaches_nothing_or_leaves_the_directory_is_refused() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &root_including(
            "\"parts/*.toml\", \"absent.toml\", \"../outside.toml\", \"*/a.toml\", \
             \"parts/[ab].toml\", \"a*b*.toml\"",
        ),
    );
    std::fs::create_dir_all(scratch.path().join("parts")).expect("create parts/");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:12: error[empty-include]: include `parts/*.toml` matches no file",
            "bc-telemetry.toml:1:28: error[invalid-include]: include `absent.toml` names no file",
            "bc-telemetry.toml:1:43: error[invalid-include]: include `../outside.toml` must stay \
             inside the directory it is taken from: no `..`, no leading `/` and no empty segment",
            "bc-telemetry.toml:1:62: error[invalid-include]: include `*/a.toml` has a `*` outside \
             its last path segment",
            "bc-telemetry.toml:1:74: error[invalid-include]: include `parts/[ab].toml` uses glob \
             syntax beyond `*`",
            "bc-telemetry.toml:1:93: error[invalid-include]: include `a*b*.toml` has more than \
             one `*`",
        ],
    );
}

#[test]
fn one_file_reached_twice_is_refused_rather_than_merged() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &root_including("\"parts/a.toml\", \"parts/*.toml\", \"*.toml\""),
    );
    scratch.write("parts/a.toml", HEADER);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:28: error[invalid-include]: include `parts/*.toml` reaches \
             `parts/a.toml`, which is already part of this module",
            "bc-telemetry.toml:1:44: error[invalid-include]: include `*.toml` reaches \
             `bc-telemetry.toml`, which is already part of this module",
        ],
    );
}

#[test]
fn a_pattern_reaching_a_file_of_no_definition_format_is_refused() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", &root_including("\"notes/*\""));
    scratch.write("notes/todo.txt", "not a definition\n");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:12: error[unsupported-format]: include `notes/*` reaches \
           `notes/todo.txt`; a definition file is `.toml` or `.json`",
        ],
    );
}

#[test]
fn an_included_file_without_its_header_is_refused() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", &root_including("\"more.toml\""));
    scratch.write(
        "more.toml",
        "[span.checkout]\ndescription = \"A checkout\"\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "more.toml:1:1: error[missing-field]: the definition file `more.toml` is missing its \
           required `module`",
        ],
    );
}

/// A pattern matches a name that starts with the text before its `*` and ends
/// with the text after it — as two distinct stretches, so `a*a.toml` needs at
/// least `aa.toml` — and only regular files.
#[test]
fn a_wildcard_matches_files_by_prefix_and_suffix_and_nothing_else() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &root_including("\"parts/metrics-*.toml\", \"edge/a*a.toml\""),
    );
    scratch.write(
        "parts/metrics-http.toml",
        &format!(
            "{HEADER}\n[metric.requests]\ndescription = \"Requests\"\ntype = \"counter\"\n\
             unit = \"none\"\n"
        ),
    );
    // Neighbours the pattern must pass over: another prefix, another suffix, and
    // a directory whose name matches.
    scratch.write("parts/logs-http.toml", "this file is never read\n");
    scratch.write("parts/metrics-http.json", "this file is never read\n");
    scratch.write(
        "parts/metrics-dir.toml/inner.toml",
        "this file is never read\n",
    );
    // `a.toml` starts with `a` and ends with `a.toml`, but only by sharing its `a`.
    scratch.write("edge/a.toml", "this file is never read\n");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &["bc-telemetry.toml:1:36: error[empty-include]: include `edge/a*a.toml` matches no file"],
    );
}

/// A module's include is a path from that module's directory, not the root's.
#[test]
fn a_modules_include_is_taken_from_its_own_directory() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", HEADER);
    scratch.write(
        "billing/bc-telemetry.toml",
        "include = [\"parts/spans.toml\"]\n\n[module]\nname = \"billing\"\nschema_version = 1\n",
    );
    scratch.write(
        "billing/parts/spans.toml",
        "[module]\nname = \"billing\"\nschema_version = 1\n\n\
         [span.charge]\ndescription = \"A charge\"\nevents = [\"declined\", \"declined\"]\n",
    );
    // The same path from the root names another file, which is never read.
    scratch.write("parts/spans.toml", "this file is never read\n");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "billing/parts/spans.toml:7:23: error[duplicate-entry]: event `declined` appears \
             twice in `events` on span `charge`",
        ],
    );
}

/// A file belongs to the module whose directory it is in. An include that
/// reaches into a subdirectory holding its own definition file would give one
/// file to two modules — whether it names the file, matches it, or passes
/// through that directory to something deeper.
#[test]
fn an_include_may_not_reach_into_another_modules_directory() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &root_including("\"billing/extra.toml\", \"billing/*.json\", \"billing/deep/more.toml\""),
    );
    scratch.write(
        "billing/bc-telemetry.toml",
        "[module]\nname = \"billing\"\nschema_version = 1\n",
    );
    scratch.write("billing/extra.toml", HEADER);
    scratch.write(
        "billing/spans.json",
        "{\"module\": {\"name\": \"shop\", \"schema_version\": 1}}\n",
    );
    scratch.write("billing/deep/more.toml", HEADER);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:12: error[invalid-include]: include `billing/extra.toml` \
             reaches `billing/extra.toml`, which is inside `billing` — a module of its own, \
             whose files are its alone",
            "bc-telemetry.toml:1:34: error[invalid-include]: include `billing/*.json` reaches \
             `billing/spans.json`, which is inside `billing` — a module of its own, whose files \
             are its alone",
            "bc-telemetry.toml:1:52: error[invalid-include]: include `billing/deep/more.toml` \
             reaches `billing/deep/more.toml`, which is inside `billing` — a module of its own, \
             whose files are its alone",
        ],
    );
}

/// What the walk skips is what it will not *find*. A file a module names is read
/// wherever it is: a hidden directory holds no module, and may well hold a part
/// of one.
#[test]
fn an_include_may_name_a_file_in_a_directory_the_walk_does_not_enter() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &format!(
            "{}\n[query.checkouts]\ndescription = \"Checkouts\"\nmeasure = \"span.checkout\"\n\
             aggregation = \"count\"\nwindow = \"1m\"\n",
            root_including("\".parts/spans.toml\"")
        ),
    );
    // The query above is valid only if this file was read.
    scratch.write(
        ".parts/spans.toml",
        &format!("{HEADER}\n[span.checkout]\ndescription = \"A checkout\"\n"),
    );
    assert_answer(
        &run_in(scratch.path(), &[]),
        "bc-telemetry.toml: valid, 1 module; nothing to generate (the root file has no \
         `[generate]` table)\n",
    );
}
