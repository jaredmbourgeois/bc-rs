//! `bc-telemetry-gen`: the process's arguments and streams handed to
//! [`bc_telemetry_gen::run`], and its status returned as the exit code.
#![warn(clippy::let_underscore_must_use, clippy::allow_attributes)]

use std::io::{self, Write};
use std::process::ExitCode;

use bc_telemetry_gen::ExitStatus;

fn main() -> ExitCode {
    let mut stderr = io::stderr().lock();
    let mut arguments = Vec::new();
    for argument in std::env::args_os().skip(1) {
        match argument.into_string() {
            Ok(argument) => arguments.push(argument),
            Err(argument) => {
                return refuse(
                    &mut stderr,
                    &format!(
                        "error: argument `{}` is not UTF-8",
                        argument.to_string_lossy()
                    ),
                    ExitStatus::Usage,
                );
            }
        }
    }
    let current_directory = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            return refuse(
                &mut stderr,
                &format!("error: cannot read the current directory: {error}"),
                ExitStatus::Findings,
            );
        }
    };
    let status = bc_telemetry_gen::run(
        arguments,
        &current_directory,
        &mut io::stdout().lock(),
        &mut stderr,
    );
    ExitCode::from(status.code())
}

/// A refusal before the generator could start: the message on stderr, and the
/// status it ends with.
fn refuse(stderr: &mut dyn Write, message: &str, status: ExitStatus) -> ExitCode {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "stderr is the diagnostic channel; a failed write to it has nowhere left to report"
    )]
    let _ = writeln!(stderr, "{message}");
    ExitCode::from(status.code())
}
