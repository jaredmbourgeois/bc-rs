//! What every module of the suite shares: an in-process run, the fixtures, and
//! a scratch directory per test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bc_telemetry_gen::{ExitStatus, run};

/// One run's status and both of its streams.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Runs the generator in process, as its command line would from `directory`.
pub(crate) fn run_in(directory: &Path, arguments: &[&str]) -> Outcome {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(
        arguments.iter().map(|argument| (*argument).to_string()),
        directory,
        &mut stdout,
        &mut stderr,
    );
    Outcome {
        status,
        stdout: String::from_utf8(stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(stderr).expect("stderr is UTF-8"),
    }
}

/// Asserts a run ended with findings and printed exactly `expected`, one
/// finding per line, in order — and nothing on stdout.
#[track_caller]
pub(crate) fn assert_findings(outcome: &Outcome, expected: &[&str]) {
    let printed: Vec<&str> = outcome.stderr.lines().collect();
    assert_eq!(printed, expected, "stderr was:\n{}", outcome.stderr);
    assert_eq!(outcome.status, ExitStatus::Findings);
    assert_eq!(outcome.stdout, "", "a refused run answers nothing");
}

/// Asserts a run succeeded with `expected` on stdout and nothing on stderr.
#[track_caller]
pub(crate) fn assert_answer(outcome: &Outcome, expected: &str) {
    assert_eq!(outcome.stderr, "", "stderr was:\n{}", outcome.stderr);
    assert_eq!(outcome.stdout, expected);
    assert_eq!(outcome.status, ExitStatus::Success);
}

/// A committed fixture directory.
pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A directory for one test, removed when dropped. Unique across concurrent
/// processes by the pid, across time by the clock — a pid alone is drawn again
/// by a later run, which would then meet a killed run's leftovers — and within a
/// process by a counter.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos();
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "gen-{}-{nanoseconds}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create the scratch directory");
        Self { path }
    }

    /// A scratch copy of a committed fixture, so a test may change it.
    pub(crate) fn of_fixture(name: &str) -> Self {
        let scratch = Self::new();
        copy_tree(&fixture(name), &scratch.path);
        scratch
    }

    /// A scratch copy of the canonical tree with every generation selector
    /// taken off its root, so it generates its manifest and nothing else. A
    /// suite about the vocabulary — how it may evolve, whose files are whose —
    /// uses this, and is not rewritten each time the canonical tree learns to
    /// generate another artifact.
    pub(crate) fn of_the_canonical_vocabulary() -> Self {
        let scratch = Self::of_fixture("canonical");
        scratch.edit("bc-telemetry.toml", "dialects = [\"postgresql\"]\n", "");
        scratch.edit("bc-telemetry.toml", "exports = [\"grafana\"]\n", "");
        scratch.edit("bc-telemetry.toml", "languages = [\"cpp\", \"rust\"]\n", "");
        scratch.edit(
            "bc-telemetry.toml",
            "[generate.grafana]\ndatasource = \"telemetry\"\n",
            "",
        );
        scratch.remove_every_generated_tree();
        let outcome = run_in(scratch.path(), &[]);
        assert_answer(
            &outcome,
            "generated/telemetry: wrote 1, removed 0, unchanged 0\n",
        );
        scratch
    }

    /// Removes everything a canonical-shaped tree generates: the root's
    /// `output`, and the same path under each other module's directory.
    pub(crate) fn remove_every_generated_tree(&self) {
        for directory in [
            "generated",
            "services/payments/generated",
            "services/payments/ledger/generated",
        ] {
            if self.exists(directory) {
                self.remove_tree(directory);
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn write(&self, relative: &str, contents: &str) {
        let target = self.path.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create the parent directory");
        }
        fs::write(target, contents).expect("write the file");
    }

    pub(crate) fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path.join(relative)).expect("read the file")
    }

    pub(crate) fn exists(&self, relative: &str) -> bool {
        self.path.join(relative).exists()
    }

    pub(crate) fn remove(&self, relative: &str) {
        fs::remove_file(self.path.join(relative)).expect("remove the file");
    }

    /// Removes a directory and everything in it — a module's, to make it vanish.
    pub(crate) fn remove_tree(&self, relative: &str) {
        fs::remove_dir_all(self.path.join(relative)).expect("remove the directory");
    }

    /// Moves a file or a directory, creating the directories above its new place.
    pub(crate) fn rename(&self, from: &str, to: &str) {
        let target = self.path.join(to);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create the parent directory");
        }
        fs::rename(self.path.join(from), target).expect("move the entry");
    }

    /// Replaces `from` with `to` in a file, refusing a plant that would change
    /// nothing — a test whose edit missed proves nothing about what it planted.
    #[track_caller]
    pub(crate) fn edit(&self, relative: &str, from: &str, to: &str) {
        let text = self.read(relative);
        assert!(
            text.contains(from),
            "`{relative}` has no `{from}` to replace"
        );
        self.write(relative, &text.replacen(from, to, 1));
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best-effort teardown: the directory sits under cargo's own temporary
        // root, and its name is never drawn again, so a leftover collides with
        // nothing.
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create the copy's directory");
    for entry in fs::read_dir(from).expect("list the fixture") {
        let entry = entry.expect("read the fixture's entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("read the entry's type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy the fixture's file");
        }
    }
}
