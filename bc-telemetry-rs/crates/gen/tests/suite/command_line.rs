//! The command line's contract: the answer on stdout, diagnostics on stderr, a
//! status for each way a run ends, and a payload that did not reach its reader
//! never reported as a success.

use std::io::{self, Write};
use std::path::Path;

use bc_telemetry_gen::{ExitStatus, run};

use crate::canonical::GENERATED_FILES;
use crate::support::{Scratch, assert_answer, assert_findings, fixture, run_in};

#[test]
fn help_and_version_are_answers() {
    let help = run_in(Path::new("."), &["--help"]);
    assert_eq!(help.status, ExitStatus::Success);
    assert!(
        help.stdout
            .starts_with("usage: bc-telemetry-gen [--file <path>] [--check]\n")
    );
    assert_eq!(help.stderr, "");

    let version = run_in(Path::new("."), &["--version"]);
    assert_eq!(version.status, ExitStatus::Success);
    assert_eq!(
        version.stdout,
        format!("bc-telemetry-gen {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_malformed_command_line_is_a_usage_error_on_stderr() {
    for (arguments, problem) in [
        (
            &["--frobnicate"][..],
            "error: unknown argument `--frobnicate`",
        ),
        (&["--file"][..], "error: `--file` needs a path"),
        (
            &["--file", "a.toml", "--file", "b.toml"][..],
            "error: `--file` is given twice",
        ),
    ] {
        let outcome = run_in(Path::new("."), arguments);
        assert_eq!(outcome.status, ExitStatus::Usage, "{arguments:?}");
        assert_eq!(outcome.stdout, "");
        assert!(
            outcome.stderr.starts_with(&format!("{problem}\n\nusage: ")),
            "stderr was:\n{}",
            outcome.stderr
        );
    }
    assert_eq!(ExitStatus::Usage.code(), 2);
}

#[test]
fn with_no_file_named_the_default_must_be_there_and_unambiguous() {
    let scratch = Scratch::new();
    let outcome = run_in(scratch.path(), &[]);
    assert_eq!(outcome.status, ExitStatus::Findings);
    assert!(
        outcome.stderr.starts_with(
            "error[no-definition]: no `bc-telemetry.toml` or `bc-telemetry.json` in `"
        )
    );

    scratch.write("bc-telemetry.toml", "");
    scratch.write("bc-telemetry.json", "{}");
    let outcome = run_in(scratch.path(), &[]);
    assert_eq!(outcome.status, ExitStatus::Findings);
    assert!(outcome.stderr.starts_with(
        "error[ambiguous-definition]: both `bc-telemetry.toml` and `bc-telemetry.json` are in `"
    ));
}

/// `--file` chooses a root directory by naming the definition file in it; the
/// file's name is fixed, because every module's is found by it.
#[test]
fn a_named_file_carries_a_definition_files_name_and_must_exist() {
    let scratch = Scratch::new();
    scratch.write("telemetry.yaml", "module: {}\n");
    scratch.write(
        "defs.toml",
        "[module]\nname = \"shop\"\nschema_version = 1\n",
    );
    for name in ["telemetry.yaml", "defs.toml", "BC-Telemetry.toml"] {
        assert_findings(
            &run_in(scratch.path(), &["--file", name]),
            &[&format!(
                "error[misnamed-definition]: `{name}` is not a definition file's name; the root \
                 definition is named `bc-telemetry.toml` or `bc-telemetry.json`, as every \
                 module's is"
            )],
        );
    }

    let no_directory = run_in(scratch.path(), &["--file", "absent/bc-telemetry.toml"]);
    assert_eq!(no_directory.status, ExitStatus::Findings);
    assert!(
        no_directory
            .stderr
            .starts_with("error[io]: cannot read `absent`: "),
        "stderr was:\n{}",
        no_directory.stderr
    );
    let no_file = run_in(scratch.path(), &["--file", "bc-telemetry.json"]);
    assert_eq!(no_file.status, ExitStatus::Findings);
    assert!(
        no_file
            .stderr
            .starts_with("error[io]: cannot read `bc-telemetry.json`: "),
        "stderr was:\n{}",
        no_file.stderr
    );
}

/// Naming one of two twins does not choose between them: the directory holds
/// its definition in one syntax.
#[test]
fn naming_one_of_two_syntaxes_in_the_root_is_still_ambiguous() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", "");
    scratch.write("bc-telemetry.json", "{}");
    let outcome = run_in(scratch.path(), &["--file", "bc-telemetry.toml"]);
    assert_eq!(outcome.status, ExitStatus::Findings);
    assert!(
        outcome.stderr.starts_with(
            "error[ambiguous-definition]: both `bc-telemetry.toml` and `bc-telemetry.json` are in `"
        ),
        "stderr was:\n{}",
        outcome.stderr
    );
}

/// A root named from elsewhere is read where it is, and every path a run prints
/// is the path from where it started.
#[test]
fn a_root_named_from_another_directory_prints_paths_from_where_the_run_started() {
    let scratch = Scratch::new();
    scratch.write(
        "shop/bc-telemetry.toml",
        "[module]\nname = \"shop\"\nschema_version = 1\n\n[generate]\noutput = \"out\"\n",
    );
    scratch.write(
        "shop/billing/bc-telemetry.toml",
        "[module]\nname = \"billing\"\nschema_version = 1\nextra = true\n",
    );
    assert_findings(
        &run_in(scratch.path(), &["--file", "shop/bc-telemetry.toml"]),
        &[
            "shop/billing/bc-telemetry.toml:4:1: error[unknown-field]: unknown field `extra` on \
             `[module]`",
        ],
    );
    scratch.edit("shop/billing/bc-telemetry.toml", "extra = true\n", "");
    assert_answer(
        &run_in(scratch.path(), &["--file", "shop/bc-telemetry.toml"]),
        "shop/out: wrote 1, removed 0, unchanged 0\n",
    );
    assert!(scratch.exists("shop/out/manifest.json"));
}

/// A stdout that refuses every write, as a closed pipe does.
struct ClosedPipe;

impl Write for ClosedPipe {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "the reader has gone",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_answer_that_cannot_be_written_is_not_a_success() {
    let mut stderr = Vec::new();
    let status = run(
        ["--check".to_string()],
        &fixture("canonical"),
        &mut ClosedPipe,
        &mut stderr,
    );
    assert_eq!(status, ExitStatus::Findings);
    assert_eq!(
        String::from_utf8(stderr).expect("stderr is UTF-8"),
        "error: could not write the answer to stdout: the reader has gone\n"
    );
}

/// The binary itself: arguments in, the library's status out as the exit code.
#[test]
fn the_binary_exits_with_the_status_run_returns() {
    let binary = env!("CARGO_BIN_EXE_bc-telemetry-gen");
    let checked = std::process::Command::new(binary)
        .arg("--check")
        .current_dir(fixture("canonical"))
        .output()
        .expect("run the binary");
    assert_eq!(checked.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout),
        format!("generated/telemetry: fresh ({GENERATED_FILES} files)\n")
    );

    let refused = std::process::Command::new(binary)
        .arg("--frobnicate")
        .output()
        .expect("run the binary");
    assert_eq!(refused.status.code(), Some(2));
}

/// An argument that is not UTF-8 cannot name anything a definition writes, and
/// is refused before the generator starts rather than mangled into a path.
#[cfg(unix)]
#[test]
fn the_binary_refuses_an_argument_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_bc-telemetry-gen"))
        .arg(std::ffi::OsStr::from_bytes(b"--file=\xff"))
        .output()
        .expect("run the binary");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: argument `--file=\u{fffd}` is not UTF-8\n"
    );
}
