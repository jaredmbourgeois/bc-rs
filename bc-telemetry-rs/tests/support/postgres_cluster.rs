//! A throwaway PostgreSQL cluster for one test.
//!
//! Shared by path between this workspace's two suites that need a real server:
//! the generator's, which holds its generated layout to its own corpus, and the
//! port's, which holds the PostgreSQL exporter to the same corpus. It uses only
//! the standard library and the PostgreSQL programs themselves — `initdb`,
//! `pg_ctl`, `psql` — which `scripts/check-test-host-programs.sh` declares and
//! `scripts/verify.sh` puts on `PATH`.
//!
//! # What a cluster is, and is not
//!
//! Each test starts its own: a data directory the test made, a server that
//! listens on a Unix socket only, in a directory only this user can enter. It
//! never touches a server that is already running — there is nothing to connect
//! to but the socket it just created — and the test that started it stops it.
//!
//! The socket's directory is a second, short one, because a socket's path has a
//! hard limit near a hundred bytes that a cargo target directory can pass on
//! its own.
//!
//! # Leftovers
//!
//! A test killed at its timeout runs no destructor, so its server outlives it.
//! The next cluster to start reclaims those: a data directory is named for the
//! process that made it, and one whose process is gone is stopped and removed.
//! A directory whose process may still be alive is always left alone — a
//! wrongly spared leftover costs one more sweep, and a wrongly stopped one
//! takes a running test's database out from under it.
#![allow(
    dead_code,
    unreachable_pub,
    reason = "compiled into each suite that includes it, and each uses a different part"
)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The mark every directory this harness makes carries, so the sweep never
/// reads a name it did not write.
const MARK: &str = "bcpg";

/// Between the rows of a query's answer, and between the fields of a row:
/// control characters no value under test holds. A row may hold a newline, so
/// rows cannot be told apart by one.
const ROW_SEPARATOR: char = '\u{1c}';
pub const FIELD_SEPARATOR: char = '\u{1f}';

/// One running cluster. Stopped and removed when dropped.
pub struct Cluster {
    data_directory: PathBuf,
    socket_directory: PathBuf,
}

impl Cluster {
    /// Starts a cluster whose data directory is under `data_root` — the test's
    /// own temporary root — and whose socket is in a new private directory
    /// under `socket_root`, which must be short.
    ///
    /// # Panics
    ///
    /// When a PostgreSQL program is missing or fails: a test that needs a
    /// database has no result without one.
    #[must_use]
    pub fn start(data_root: &Path, socket_root: &Path) -> Self {
        // Tells apart two clusters one process starts in the same nanosecond.
        static NEXT: AtomicU64 = AtomicU64::new(0);

        reclaim_leftovers(data_root, socket_root);
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos();
        let name = format!(
            "{MARK}-{}-{nanoseconds}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let data_directory = data_root.join(&name);
        let socket_directory = socket_root.join(&name);
        fs::create_dir_all(data_root).expect("create the data root");
        // Created new and private: a directory that already exists is not ours,
        // and nobody else may reach the socket inside.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&socket_directory)
            .expect("create the socket directory");

        let cluster = Self {
            data_directory,
            socket_directory,
        };
        run(
            Command::new("initdb")
                .arg("--pgdata")
                .arg(&cluster.data_directory)
                .args(["--username", "postgres", "--auth", "trust"])
                .args(["--encoding", "UTF8", "--locale", "C", "--no-sync"]),
            "initdb",
        );
        let options = format!(
            "-c listen_addresses='' -c unix_socket_directories='{}' -c fsync=off -c timezone=UTC",
            cluster.socket_directory.display()
        );
        run(
            Command::new("pg_ctl")
                .arg("--pgdata")
                .arg(&cluster.data_directory)
                .arg("--log")
                .arg(cluster.data_directory.join("server.log"))
                .args(["-o", &options, "--wait", "start"]),
            "pg_ctl start",
        );
        cluster
    }

    /// The server's version, as it reports it — so a run says which PostgreSQL
    /// it actually reached.
    #[must_use]
    pub fn version(&self) -> String {
        self.rows("SHOW server_version").join("")
    }

    /// The directory holding the cluster's socket: its address, for a client
    /// that takes one as a host, as Grafana's data source does.
    #[must_use]
    pub fn socket_directory(&self) -> &Path {
        &self.socket_directory
    }

    /// libpq's keyword/value connection string for this cluster.
    #[must_use]
    pub fn connection_string(&self) -> String {
        format!(
            "host={} user=postgres dbname=postgres",
            self.socket_directory.display()
        )
    }

    fn psql(&self) -> Command {
        let mut command = Command::new("psql");
        command
            .args(["--no-psqlrc", "--quiet", "--no-align", "--tuples-only"])
            .args(["--set", "ON_ERROR_STOP=1"])
            .arg("--host")
            .arg(&self.socket_directory)
            .args(["--username", "postgres", "--dbname", "postgres"])
            .arg("--record-separator")
            .arg(ROW_SEPARATOR.to_string())
            .arg("--field-separator")
            .arg(FIELD_SEPARATOR.to_string());
        command
    }

    /// Applies an SQL file.
    ///
    /// # Panics
    ///
    /// When any statement in it fails.
    pub fn apply(&self, file: &Path) {
        run(self.psql().arg("--file").arg(file), "psql --file");
    }

    /// Applies an SQL file and says whether every statement in it succeeded.
    #[must_use]
    pub fn applies(&self, file: &Path) -> bool {
        self.psql()
            .arg("--file")
            .arg(file)
            .stdin(Stdio::null())
            .output()
            .expect("run psql")
            .status
            .success()
    }

    /// Runs one statement and says whether the server refused it.
    #[must_use]
    pub fn refuses(&self, statement: &str) -> bool {
        !self
            .psql()
            .args(["--command", statement])
            .stdin(Stdio::null())
            .output()
            .expect("run psql")
            .status
            .success()
    }

    /// Runs one statement and returns its rows, each a row's fields joined by
    /// [`FIELD_SEPARATOR`].
    ///
    /// # Panics
    ///
    /// When the statement fails.
    #[must_use]
    pub fn rows(&self, statement: &str) -> Vec<String> {
        self.session(statement)
    }

    /// Runs a script — statements, and the data of any `COPY ... FROM STDIN`
    /// among them — in one session, and returns the rows it answered with.
    /// One session matters: staging tables are the session's own. A script
    /// asks one question: the rows of two answers would run together.
    ///
    /// # Panics
    ///
    /// When any statement fails.
    #[must_use]
    pub fn session(&self, script: &str) -> Vec<String> {
        let mut child = self
            .psql()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start psql");
        child
            .stdin
            .take()
            .expect("psql's stdin is piped")
            .write_all(script.as_bytes())
            .expect("write the script to psql");
        let output = child.wait_with_output().expect("wait for psql");
        assert!(
            output.status.success(),
            "psql refused the script:\n{}\nthe script was:\n{script}",
            String::from_utf8_lossy(&output.stderr)
        );
        let answer = String::from_utf8(output.stdout).expect("psql answers in UTF-8");
        // psql puts the row separator between rows and one newline after the
        // last; what is between two separators is a row, whatever it holds.
        let Some(rows) = answer.strip_suffix('\n') else {
            assert!(
                answer.is_empty(),
                "psql's answer does not end as expected: {answer:?}"
            );
            return Vec::new();
        };
        rows.split(ROW_SEPARATOR).map(str::to_string).collect()
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        stop_and_remove(&self.data_directory, &self.socket_directory);
    }
}

/// Runs a PostgreSQL program to completion and requires it to succeed.
fn run(command: &mut Command, what: &str) {
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
}

/// Stops the server whose data directory this is — the one this harness
/// started there, found by the directory's own pid file — and removes both
/// directories. Best effort throughout: this runs in a destructor and in a
/// sweep, and a leftover is reclaimed by the next cluster to start.
fn stop_and_remove(data_directory: &Path, socket_directory: &Path) {
    let _ = Command::new("pg_ctl")
        .arg("--pgdata")
        .arg(data_directory)
        .args(["--mode", "immediate", "--wait", "stop"])
        .stdin(Stdio::null())
        .output();
    let _ = fs::remove_dir_all(data_directory);
    let _ = fs::remove_dir_all(socket_directory);
}

/// Stops and removes every cluster under `data_root` whose test is gone.
fn reclaim_leftovers(data_root: &Path, socket_root: &Path) {
    let Ok(entries) = fs::read_dir(data_root) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(creator) = creator_of(&name) else {
            continue;
        };
        if creator == process::id() || is_running(creator) {
            continue;
        }
        stop_and_remove(&entry.path(), &socket_root.join(&name));
    }
}

/// The process that made the directory `name`, when this harness named it.
fn creator_of(name: &str) -> Option<u32> {
    let mut parts = name.split('-');
    if parts.next() != Some(MARK) {
        return None;
    }
    parts.next()?.parse().ok()
}

/// Whether a process with this id exists. Anything short of a clear "no" reads
/// as "yes": a leftover spared is cheap, and a live test's cluster stopped is
/// not.
fn is_running(process_id: u32) -> bool {
    Command::new("ps")
        .args(["-p", &process_id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_or(true, |status| status.success())
}
