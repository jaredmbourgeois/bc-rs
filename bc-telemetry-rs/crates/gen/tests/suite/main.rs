//! `bc-telemetry-gen`'s behavior, driven through `run` exactly as the command
//! line drives it. One test binary for the crate, a module per concern.

mod canonical;
mod command_line;
mod discovery;
mod evolution;
mod grafana;
#[cfg(unix)]
mod grafana_in_the_loop;
mod includes;
mod layout;
mod output;
mod refusals;
mod rust_adapter;
mod support;
#[cfg(unix)]
mod typescript;

/// The throwaway PostgreSQL cluster, shared by path with the port's suite: one
/// harness for the workspace's two suites that need a real server.
#[cfg(unix)]
#[path = "../../../../tests/support/postgres_cluster.rs"]
mod postgres_cluster;

/// Grafana's dashboard schema as an oracle, shared by path with the dashboard
/// model's suite.
#[path = "../../../../tests/support/json_schema.rs"]
mod json_schema;
