//! The integration tier: the canonical tree's dashboards, and the one the
//! TypeScript proof builds, provisioned into a real Grafana beside a real
//! PostgreSQL that holds the layout and the corpus. Each
//! dashboard is fetched back from Grafana's API, and every panel's query runs
//! through Grafana's own PostgreSQL data source — its macros expanded by
//! Grafana, not by this suite — and must answer with rows. A logs panel's answer
//! must also be a frame Grafana's logs panel reads as log lines.
//!
//! It needs the pinned Grafana and data source (`scripts/tool-pins.sh`), which
//! `scripts/fetch-grafana.sh` fetches, so a plain run ignores it.
//! `scripts/verify.sh` runs it with the fetched home named in
//! `BC_TELEMETRY_GRAFANA_HOME`, and says by name that it skipped it where
//! nothing is fetched; run here without that, it fails rather than passing.
//!
//! # The Grafana it starts
//!
//! Each run starts its own: a configuration, a database and a log under the
//! test's temporary root, a server answering on a Unix socket only, in a
//! directory only this user can enter, and anonymous viewers. Nothing listens on
//! a port, and nothing is fetched: Grafana's plugin preinstall is turned off, so
//! the data source is the pinned one the fetch put in the home's
//! `plugins-bundled`. The test that started it stops it. One killed at its
//! timeout runs no destructor, so the next Grafana to start stops what a gone
//! test left, by the pid file Grafana wrote — and only when that process's own
//! command line names the leftover's directory, since a pid is drawn again.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bc_json::JsonReader;

use crate::layout::{SOCKET_ROOT, with_the_corpus};
use crate::postgres_cluster::{Cluster, FIELD_SEPARATOR};
use crate::support::{Scratch, fixture};

/// Where `scripts/verify.sh` names the Grafana home it fetched. Read once, at
/// the test's entry: the one value this test is handed from outside.
const GRAFANA_HOME: &str = "BC_TELEMETRY_GRAFANA_HOME";

/// The data source every generated panel names (`[generate.grafana]` in the
/// canonical tree's root file).
const DATA_SOURCE_UID: &str = "telemetry";

/// The mark every directory this harness makes carries, so the sweep never
/// reads a name it did not write.
const MARK: &str = "bcgf";

/// How long Grafana has to start, register its data source and reach the
/// cluster — about fifteen seconds on an idle machine, many more on a loaded
/// one — polled every [`POLL_INTERVAL`]. Still a bound: a hang fails the test.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLLS_BEFORE_GIVING_UP: u32 = 480;

/// Panels with a query across the three dashboards, and the logs panels among
/// them: counted, so a panel the walk skipped fails rather than passes.
const PANELS_WITH_A_QUERY: usize = 13;
const LOGS_PANELS: usize = 4;

#[test]
#[ignore = "needs the pinned Grafana scripts/fetch-grafana.sh fetches; scripts/verify.sh runs it"]
fn every_panel_of_every_dashboard_answers_through_a_real_grafana() {
    let home = std::env::var_os(GRAFANA_HOME).map_or_else(
        || {
            panic!(
                "{GRAFANA_HOME} is not set: run scripts/verify.sh, or name the home \
                 scripts/fetch-grafana.sh prints"
            )
        },
        PathBuf::from,
    );

    let cluster = with_the_corpus();
    let (from, to) = the_span_of_the_corpus(&cluster);

    // Every dashboard in one directory, which Grafana provisions from.
    let dashboards = Scratch::new();
    let mut files = generated_dashboards();
    files.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/typescript/checkout-health.expected.json"),
    );
    let mut uids = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read a dashboard");
        let dashboard = JsonReader::new(bc_json::validate_json(&text).expect("JSON"));
        uids.push(dashboard.required_string("uid").expect("a dashboard's uid"));
        let name = file
            .file_name()
            .expect("a file name")
            .to_str()
            .expect("UTF-8");
        dashboards.write(name, &text);
    }

    let grafana = Grafana::start(
        &home,
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        Path::new(SOCKET_ROOT),
        cluster.socket_directory(),
        dashboards.path(),
    );
    println!("Grafana {}", grafana.version());
    grafana.wait_for_dashboards(&uids);

    let mut panels_with_a_query = 0;
    let mut logs_panels = 0;
    for uid in &uids {
        let answer = grafana.get(&format!("/api/dashboards/uid/{uid}"));
        let fetched = JsonReader::new(bc_json::validate_json(&answer).expect("JSON"));
        let meta = fetched
            .find_object("meta")
            .expect("a fetched dashboard's meta");
        assert!(
            meta.required_boolean("provisioned").expect("provisioned"),
            "`{uid}` is not the provisioned dashboard"
        );
        let dashboard = fetched.find_object("dashboard").expect("the dashboard");
        for panel in dashboard.required_object_array("panels").expect("panels") {
            let kind = panel.required_string("type").expect("a panel's type");
            let targets = panel.element_array("targets");
            if targets.is_empty() {
                assert_eq!(kind, "row", "`{uid}`: a {kind} panel with no query");
                continue;
            }
            for target in targets {
                let frames = grafana.query(target, &from, &to);
                let title = panel.required_string("title").expect("a panel's title");
                assert!(
                    frames.iter().any(|frame| frame.rows > 0),
                    "`{uid}` panel `{title}` answered with no rows: {frames:?}"
                );
                if kind == "logs" {
                    for frame in &frames {
                        frame.assert_grafana_reads_it_as_log_lines(uid, &title);
                    }
                    logs_panels += 1;
                }
            }
            panels_with_a_query += 1;
        }
    }
    assert_eq!(panels_with_a_query, PANELS_WITH_A_QUERY);
    assert_eq!(logs_panels, LOGS_PANELS);
}

/// The dashboards the canonical tree generates, in name order.
fn generated_dashboards() -> Vec<PathBuf> {
    let directory = fixture("canonical").join("generated/telemetry/grafana/dashboards");
    let mut files: Vec<PathBuf> = fs::read_dir(directory)
        .expect("the generated dashboards")
        .map(|entry| entry.expect("a dashboard").path())
        .collect();
    files.sort();
    files
}

/// The first and last instants the corpus wrote a record at, in Unix
/// milliseconds as Grafana's API takes a range — the last made exclusive, so
/// every record falls inside.
fn the_span_of_the_corpus(cluster: &Cluster) -> (String, String) {
    let rows = cluster.rows(
        "SELECT floor(extract(epoch FROM min(time)) * 1000)::bigint,
                floor(extract(epoch FROM max(time)) * 1000)::bigint + 1
         FROM (SELECT time FROM telemetry.spans
               UNION ALL SELECT time FROM telemetry.metrics
               UNION ALL SELECT time FROM telemetry.logs) AS every_record",
    );
    let [row] = &rows[..] else {
        panic!("one row for the corpus's span, got {rows:?}");
    };
    let (from, to) = row
        .split_once(FIELD_SEPARATOR)
        .expect("two fields: the first and last instants");
    (from.to_string(), to.to_string())
}

/// One frame of a query's answer, as far as this tier reads it.
#[derive(Debug)]
struct Frame {
    /// Each field's name and Grafana's type for it, in the frame's order.
    fields: Vec<(String, String)>,
    rows: usize,
}

impl Frame {
    /// Grafana's logs panel reads a PostgreSQL answer — a frame no log type
    /// marks — as its first time field for the timestamp, its first text field
    /// for the line, and a field named `level` for the severity
    /// (`public/app/features/logs/legacyLogsFrame.ts`, Grafana 13.2.3; re-read
    /// when the pin moves). So the line must be the record's body.
    #[track_caller]
    fn assert_grafana_reads_it_as_log_lines(&self, uid: &str, title: &str) {
        let first_of_type = |wanted: &str| {
            self.fields
                .iter()
                .find(|(_, field_type)| field_type == wanted)
                .map(|(name, _)| name.as_str())
        };
        assert_eq!(
            first_of_type("time"),
            Some("time"),
            "`{uid}` panel `{title}`: the timestamp Grafana reads: {:?}",
            self.fields
        );
        assert_eq!(
            first_of_type("string"),
            Some("body"),
            "`{uid}` panel `{title}`: the line Grafana reads: {:?}",
            self.fields
        );
        assert!(
            self.fields.iter().any(|(name, _)| name == "level"),
            "`{uid}` panel `{title}`: no severity Grafana reads: {:?}",
            self.fields
        );
    }
}

/// One running Grafana. Stopped, and its directories removed, when dropped.
struct Grafana {
    child: Child,
    directory: PathBuf,
    socket_directory: PathBuf,
}

impl Grafana {
    /// Starts the Grafana at `home` with its state under `data_root` and its
    /// socket in a new private directory under `socket_root`, which must be
    /// short; its data source reaches the cluster at `database_socket_directory`,
    /// and it provisions every dashboard in `dashboards`. Returns once the data
    /// source has reached the cluster.
    ///
    /// # Panics
    ///
    /// When Grafana cannot be started or never becomes ready: a test that needs
    /// it has no result without it.
    fn start(
        home: &Path,
        data_root: &Path,
        socket_root: &Path,
        database_socket_directory: &Path,
        dashboards: &Path,
    ) -> Self {
        // Tells apart two Grafanas one process starts in the same nanosecond.
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
        let directory = data_root.join(&name);
        let socket_directory = socket_root.join(&name);
        fs::create_dir_all(directory.join("provisioning/datasources"))
            .expect("create the data source provisioning directory");
        fs::create_dir_all(directory.join("provisioning/dashboards"))
            .expect("create the dashboard provisioning directory");
        // Created new and private: a directory that already exists is not ours,
        // and nobody else may reach the socket inside.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&socket_directory)
            .expect("create the socket directory");

        fs::write(
            directory.join("grafana.ini"),
            configuration(&directory, &socket_directory.join("grafana.sock")),
        )
        .expect("write Grafana's configuration");
        fs::write(
            directory.join("provisioning/datasources/telemetry.yaml"),
            format!(
                "apiVersion: 1\n\
                 datasources:\n  \
                   - name: telemetry\n    \
                     uid: {DATA_SOURCE_UID}\n    \
                     type: grafana-postgresql-datasource\n    \
                     url: {}\n    \
                     user: postgres\n    \
                     jsonData:\n      \
                       database: postgres\n      \
                       sslmode: disable\n",
                database_socket_directory.display()
            ),
        )
        .expect("provision the data source");
        fs::write(
            directory.join("provisioning/dashboards/generated.yaml"),
            format!(
                "apiVersion: 1\n\
                 providers:\n  \
                   - name: generated\n    \
                     type: file\n    \
                     disableDeletion: true\n    \
                     allowUiUpdates: false\n    \
                     options:\n      \
                       path: {}\n",
                dashboards.display()
            ),
        )
        .expect("provision the dashboards");

        let output =
            fs::File::create(directory.join("grafana.out")).expect("create Grafana's output");
        let child = Command::new(home.join("bin/grafana"))
            .arg("server")
            // Grafana finds its home only from this or its working directory.
            .arg("--homepath")
            .arg(home)
            .arg("--config")
            .arg(directory.join("grafana.ini"))
            .arg("--pidfile")
            .arg(directory.join("grafana.pid"))
            .stdin(Stdio::null())
            .stdout(output.try_clone().expect("share Grafana's output"))
            .stderr(output)
            .spawn()
            .unwrap_or_else(|error| {
                panic!("start {}: {error}", home.join("bin/grafana").display())
            });

        let mut grafana = Self {
            child,
            directory,
            socket_directory,
        };
        grafana.wait_until_the_data_source_reaches_the_cluster();
        grafana
    }

    /// Grafana answers `/api/health` before it has registered its data source
    /// plugins, so readiness is the data source's own health check, which
    /// answers OK only once its plugin is up and it has reached the cluster.
    fn wait_until_the_data_source_reaches_the_cluster(&mut self) {
        let path = format!("/api/datasources/uid/{DATA_SOURCE_UID}/health");
        let mut last_answer = String::from("(nothing yet)");
        for _ in 0..POLLS_BEFORE_GIVING_UP {
            if let Some(status) = self.child.try_wait().expect("ask after Grafana") {
                panic!(
                    "Grafana exited ({status}) before it was ready:\n{}",
                    self.output_tail()
                );
            }
            if let Some(answer) = self.try_get(&path) {
                if answer.contains("\"status\":\"OK\"") {
                    return;
                }
                last_answer = answer;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        panic!(
            "Grafana's data source did not reach the cluster in time; it last answered \
             {last_answer}\n{}",
            self.output_tail()
        );
    }

    /// Waits until Grafana has provisioned a dashboard for every uid: it
    /// provisions in the background after it starts answering.
    fn wait_for_dashboards(&self, uids: &[String]) {
        let mut missing: Vec<&String> = uids.iter().collect();
        for _ in 0..POLLS_BEFORE_GIVING_UP {
            let found = self.get("/api/search?type=dash-db");
            missing.retain(|uid| !found.contains(&format!("\"uid\":\"{uid}\"")));
            if missing.is_empty() {
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        panic!(
            "Grafana never provisioned {missing:?}; a dashboard it refuses is named in its \
             log:\n{}",
            self.output_tail()
        );
    }

    /// Grafana's version, as it reports it — so a run says which Grafana it
    /// actually reached.
    fn version(&self) -> String {
        let answer = self.get("/api/health");
        let health = JsonReader::new(bc_json::validate_json(&answer).expect("JSON"));
        health.required_string("version").expect("a version")
    }

    /// Runs one panel target, as Grafana stores it, over `from`..`to`, and
    /// returns its frames, requiring Grafana to have answered without an error.
    fn query(&self, target: &str, from: &str, to: &str) -> Vec<Frame> {
        let request = format!("{{\"from\":\"{from}\",\"to\":\"{to}\",\"queries\":[{target}]}}");
        let answer = self.post("/api/ds/query", &request);
        let document = JsonReader::new(bc_json::validate_json(&answer).expect("JSON"));
        let results = document.find_object("results").expect("a query's results");
        let target_reader = JsonReader::new(bc_json::validate_json(target).expect("a target"));
        let reference = target_reader.required_string("refId").expect("a refId");
        let result = results
            .find_object(&reference)
            .unwrap_or_else(|| panic!("no result for `{reference}`: {answer}"));
        assert!(
            !result.has_field("error"),
            "Grafana refused the query: {answer}\nthe target was: {target}"
        );
        result
            .required_object_array("frames")
            .expect("frames")
            .iter()
            .map(|frame| {
                let schema = frame.find_object("schema").expect("a frame's schema");
                let fields = schema
                    .required_object_array("fields")
                    .expect("a frame's fields")
                    .iter()
                    .map(|field| {
                        (
                            field.required_string("name").expect("a field's name"),
                            field.required_string("type").expect("a field's type"),
                        )
                    })
                    .collect();
                let data = frame.find_object("data").expect("a frame's data");
                let columns = data.element_array("values");
                let rows = columns
                    .first()
                    .map_or(0, |column| JsonReader::extract_array_elements(column).len());
                Frame { fields, rows }
            })
            .collect()
    }

    /// `path`'s answer, requiring Grafana to have answered with success.
    fn get(&self, path: &str) -> String {
        self.curl(path, &[])
    }

    /// The answer to `body`, posted as JSON to `path`, requiring success.
    fn post(&self, path: &str, body: &str) -> String {
        self.curl(
            path,
            &[
                "--request",
                "POST",
                "--header",
                "Content-Type: application/json",
                "--data-binary",
                body,
            ],
        )
    }

    fn curl(&self, path: &str, arguments: &[&str]) -> String {
        let output = self
            .curl_command(path)
            .arg("--fail-with-body")
            .args(arguments)
            .output()
            .expect("run curl");
        assert!(
            output.status.success(),
            "Grafana did not answer {path} with success: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("Grafana answers in UTF-8")
    }

    /// `path`'s answer, whatever its status, or nothing while Grafana is not
    /// yet listening.
    fn try_get(&self, path: &str) -> Option<String> {
        let output = self.curl_command(path).output().expect("run curl");
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn curl_command(&self, path: &str) -> Command {
        let mut command = Command::new("curl");
        command
            .args(["--silent", "--show-error", "--max-time", "30"])
            .arg("--unix-socket")
            .arg(self.socket_directory.join("grafana.sock"))
            // The host is a placeholder: over a Unix socket only the path is read.
            .arg(format!("http://grafana{path}"))
            .stdin(Stdio::null());
        command
    }

    /// The end of what Grafana printed, and of its log, for a failure message.
    fn output_tail(&self) -> String {
        let mut tail = String::new();
        for file in ["grafana.out", "logs/grafana.log"] {
            let text = fs::read_to_string(self.directory.join(file)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(40);
            tail.push_str("--- ");
            tail.push_str(file);
            tail.push_str(", last lines:\n");
            tail.push_str(&lines[start..].join("\n"));
            tail.push('\n');
        }
        tail
    }
}

impl Drop for Grafana {
    fn drop(&mut self) {
        // Best effort throughout: this runs in a destructor, and a leftover is
        // reclaimed by the next Grafana to start.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
        let _ = fs::remove_dir_all(&self.socket_directory);
    }
}

/// Grafana's configuration: its state under `directory`, a Unix socket only,
/// anonymous viewers, nothing reported or fetched.
fn configuration(directory: &Path, socket: &Path) -> String {
    format!(
        "[paths]\n\
         data = {directory}/data\n\
         logs = {directory}/logs\n\
         plugins = {directory}/plugins\n\
         provisioning = {directory}/provisioning\n\
         \n\
         [server]\n\
         protocol = socket\n\
         socket = {socket}\n\
         \n\
         [auth.anonymous]\n\
         enabled = true\n\
         \n\
         [analytics]\n\
         reporting_enabled = false\n\
         check_for_updates = false\n\
         check_for_plugin_updates = false\n\
         \n\
         [plugins]\n\
         preinstall_disabled = true\n\
         \n\
         [log]\n\
         mode = file\n",
        directory = directory.display(),
        socket = socket.display()
    )
}

/// Stops and removes every Grafana under `data_root` whose test is gone.
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
        stop_a_leftover(&entry.path());
        let _ = fs::remove_dir_all(entry.path());
        let _ = fs::remove_dir_all(socket_root.join(&name));
    }
}

/// Stops the Grafana a gone test left in `directory`, found by the pid file
/// Grafana wrote there — and only when that process's command line names the
/// directory, since a pid outlives its process and is drawn again.
fn stop_a_leftover(directory: &Path) {
    let Ok(text) = fs::read_to_string(directory.join("grafana.pid")) else {
        return;
    };
    let Ok(grafana) = text.trim().parse::<u32>() else {
        return;
    };
    let Ok(output) = Command::new("ps")
        .args(["-p", &grafana.to_string(), "-o", "command="])
        .stdin(Stdio::null())
        .output()
    else {
        return;
    };
    let command_line = String::from_utf8_lossy(&output.stdout);
    if command_line.contains(&directory.display().to_string()) {
        let _ = Command::new("kill")
            .args(["-KILL", &grafana.to_string()])
            .stdin(Stdio::null())
            .status();
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
/// as "yes": a leftover spared is cheap, and a live test's Grafana stopped is
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
