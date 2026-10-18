//! The command line: arguments in, an [`ExitStatus`] out.
//!
//! stdout carries the answer and nothing else; every diagnostic goes to stderr.
//! [`run`] returns its status rather than exiting, and takes both streams as
//! arguments, so every path through it — refusals included — is testable in
//! process.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::adapters::{Language, cpp, rust};
use crate::diagnostics::{Code, Diagnostics, Reported};
use crate::discover::{self, DEFINITION_FILE_NAMES};
use crate::evolution;
use crate::front::{self, Format};
use crate::grafana;
use crate::layout::{self, RenderError};
use crate::load::{self, LoadedModule, ModuleLocation};
use crate::lower;
use crate::manifest::{self, GENERATOR, MANIFEST_FILE_NAME, PreviousManifest, Retired};
use crate::model::{Dialect, Tree};
use crate::output::{self, Difference, GeneratedTree};
use crate::paths;
use crate::root::{self, GenerateConfig, RootConfig};
use crate::source::Sources;
use crate::validate::{self, Declared};

/// How a run ended. The process exit code is [`ExitStatus::code`] and nothing
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    /// The definitions are valid, and the generated tree was written or found
    /// fresh.
    Success,
    /// The definitions or the generated tree have findings, each reported on
    /// stderr — or the answer could not be written to stdout.
    Findings,
    /// The command line itself was malformed; the usage went to stderr.
    Usage,
}

impl ExitStatus {
    /// The process exit code: 0, 1 or 2.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Findings => 1,
            Self::Usage => 2,
        }
    }
}

const USAGE: &str = "\
usage: bc-telemetry-gen [--file <path>] [--check]

Validates every telemetry definition under a root directory and generates what the
root's [generate] table asks for.

  --file <path>  the root definition file, named bc-telemetry.toml or
                 bc-telemetry.json (default: the one in the current directory)
  --check        compare the committed generated tree with what the definitions
                 generate, and write nothing
  --help         print this message
  --version      print the generator's name and version

Exit status: 0 valid and written (or fresh), 1 findings, 2 a malformed command line.
";

enum Command {
    Generate { file: Option<PathBuf>, check: bool },
    Help,
    Version,
}

/// Runs the generator as its command line would, with `arguments` excluding
/// the program name and relative paths resolved against `current_directory`.
pub fn run(
    arguments: impl IntoIterator<Item = String>,
    current_directory: &Path,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ExitStatus {
    let command = match parse_arguments(arguments) {
        Ok(command) => command,
        Err(problem) => {
            report(stderr, &format!("error: {problem}\n\n{USAGE}"));
            return ExitStatus::Usage;
        }
    };
    match command {
        Command::Help => answer(stdout, stderr, USAGE),
        Command::Version => answer(stdout, stderr, &format!("{GENERATOR}\n")),
        Command::Generate { file, check } => {
            let mut sources = Sources::default();
            let mut diagnostics = Diagnostics::default();
            let summary = generate(
                file.as_deref(),
                check,
                current_directory,
                &mut sources,
                &mut diagnostics,
            );
            // The findings are the verdict, read in this one place: a run that
            // reported any fails, whatever else it produced.
            if diagnostics.is_empty() {
                if let Some(summary) = summary {
                    return answer(stdout, stderr, &summary);
                }
                debug_assert!(false, "a run produced no answer and no finding");
                diagnostics.unplaced(
                    Code::Internal,
                    "the run produced no answer and no finding — a defect in bc-telemetry-gen",
                );
            }
            report(stderr, &diagnostics.render(&sources));
            ExitStatus::Findings
        }
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut file = None;
    let mut check = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" => return Ok(Command::Version),
            "--check" => check = true,
            "--file" => {
                let Some(path) = arguments.next() else {
                    return Err("`--file` needs a path".to_string());
                };
                if file.replace(PathBuf::from(path)).is_some() {
                    return Err("`--file` is given twice".to_string());
                }
            }
            unknown => return Err(format!("unknown argument `{unknown}`")),
        }
    }
    Ok(Command::Generate { file, check })
}

/// Writes the answer to stdout. A payload that did not reach its reader is a
/// failed run, never a green one: a truncated answer must not exit 0.
fn answer(stdout: &mut dyn Write, stderr: &mut dyn Write, text: &str) -> ExitStatus {
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitStatus::Success,
        Err(error) => {
            report(
                stderr,
                &format!("error: could not write the answer to stdout: {error}\n"),
            );
            ExitStatus::Findings
        }
    }
}

fn report(stderr: &mut dyn Write, text: &str) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "stderr is the diagnostic channel; a failed write to it has nowhere left to report"
    )]
    let _ = stderr.write_all(text.as_bytes());
}

/// Everything after the arguments: read and validate the tree, then generate
/// or check. Returns the answer for stdout; findings are in `diagnostics`.
fn generate(
    file: Option<&Path>,
    check: bool,
    current_directory: &Path,
    sources: &mut Sources,
    diagnostics: &mut Diagnostics,
) -> Option<String> {
    let root = Root::locate(file, current_directory, diagnostics)?;
    let (tree, config) = definitions(&root, sources, diagnostics)?;
    let Some(generate) = &config.generate else {
        let modules = if tree.modules.len() == 1 {
            "module"
        } else {
            "modules"
        };
        return Some(format!(
            "{}: valid, {} {modules}; nothing to generate (the root file has no `[generate]` \
             table)\n",
            root.display_directory.join(root.file_name).display(),
            tree.modules.len()
        ));
    };
    let output = generate.output.value.as_str();
    let manifest_path = paths::under(output, MANIFEST_FILE_NAME);

    let previous = previous_manifest(&root, &manifest_path, sources, diagnostics).ok()?;
    // The directories whose contents are the generator's: this tree's, and
    // those of the tree the committed manifest describes — a module that moved
    // or went, and an `output` that changed, left files where they were.
    let mut owned = BTreeSet::new();
    owned.insert(output.to_string());
    for module in tree.modules.values() {
        owned.insert(paths::under(&module.directory, output));
    }
    if let Some(previous) = &previous {
        owned.extend(previous.owned_directories());
    }
    let committed = match output::read_committed(&root.directory, &root.display_directory, &owned) {
        Ok(committed) => committed,
        Err(problem) => {
            diagnostics.unplaced(Code::Io, problem);
            return None;
        }
    };

    let retired = evolution::check(previous.as_ref(), &tree, &config.retired, diagnostics);
    let generated = generated_tree(&tree, &retired, generate, &manifest_path, diagnostics)?;
    let previously_generated = match previous {
        Some(previous) => {
            let mut files = previous.files;
            // The manifest that was read is the generator's wherever it sits:
            // after `output` changes, it sits where the new one puts it.
            files.insert(manifest_path);
            files
        }
        None => BTreeSet::new(),
    };
    let difference = output::compare(&generated, &committed, &previously_generated);
    let output_display = root.display_directory.join(output);

    if check {
        report_difference(&difference, &root, diagnostics);
        return diagnostics.is_empty().then(|| {
            let files = if generated.len() == 1 {
                "file"
            } else {
                "files"
            };
            format!(
                "{}: fresh ({} {files})\n",
                output_display.display(),
                generated.len()
            )
        });
    }
    for path in &difference.unexpected {
        diagnostics.unplaced(
            Code::OutputUnexpected,
            format!(
                "`{}` is in a directory the generator owns, and no run generated it; nothing was \
                 written — move it out, or choose another `output`",
                root.display_of(path)
            ),
        );
    }
    if !diagnostics.is_empty() {
        return None;
    }
    if let Err(error) = output::apply(&root.directory, &generated, &difference, &owned) {
        diagnostics.unplaced(
            Code::Io,
            format!("cannot write `{}`: {error}", output_display.display()),
        );
        return None;
    }
    Some(format!(
        "{}: wrote {}, removed {}, unchanged {}\n",
        output_display.display(),
        difference.changed.len() + difference.missing.len(),
        difference.stale.len(),
        difference.unchanged
    ))
}

/// The root directory — as a path to open and a path to print — and the name
/// of the definition file in it.
struct Root {
    directory: PathBuf,
    display_directory: PathBuf,
    file_name: &'static str,
}

impl Root {
    /// The root `--file` names, or the current directory's.
    fn locate(
        file: Option<&Path>,
        current_directory: &Path,
        diagnostics: &mut Diagnostics,
    ) -> Option<Self> {
        let Some(file) = file else {
            let file_name =
                discover::root_definition(current_directory, Path::new(""), None, diagnostics)?;
            return Some(Self {
                directory: current_directory.to_path_buf(),
                display_directory: PathBuf::new(),
                file_name,
            });
        };
        let named = file
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| {
                DEFINITION_FILE_NAMES
                    .into_iter()
                    .find(|definition_name| *definition_name == name)
            });
        let Some(named) = named else {
            let [toml, json] = DEFINITION_FILE_NAMES;
            diagnostics.unplaced(
                Code::MisnamedDefinition,
                format!(
                    "`{}` is not a definition file's name; the root definition is named `{toml}` \
                     or `{json}`, as every module's is",
                    file.display()
                ),
            );
            return None;
        };
        let display_directory = match file.parent() {
            Some(parent) => parent.to_path_buf(),
            None => PathBuf::new(),
        };
        let directory = current_directory.join(&display_directory);
        let file_name =
            discover::root_definition(&directory, &display_directory, Some(named), diagnostics)?;
        Some(Self {
            directory,
            display_directory,
            file_name,
        })
    }

    /// A path from the root directory, as a finding prints it.
    fn display_of(&self, from_root: &str) -> String {
        self.display_directory.join(from_root).display().to_string()
    }
}

/// Reads, lowers and validates every definition under the root. `None` when
/// anything had findings.
fn definitions(
    root: &Root,
    sources: &mut Sources,
    diagnostics: &mut Diagnostics,
) -> Option<(Tree, RootConfig)> {
    let root_display = root.display_directory.join(root.file_name);
    let root_file = load::read_definition(
        &root.directory.join(root.file_name),
        &root_display,
        format_of(root.file_name)?,
        sources,
        diagnostics,
    )?;
    let context = format!("the definition file `{}`", root_display.display());
    let config = root::root_config(&root_file, &context, diagnostics);
    let output = config
        .generate
        .as_ref()
        .map(|generate| generate.output.value.as_str());

    let sites = discover::modules_below(
        &root.directory,
        &root.display_directory,
        output,
        diagnostics,
    );
    let module_directories: BTreeSet<String> =
        sites.iter().map(|site| site.directory.clone()).collect();
    let location_of = |directory, file_name| ModuleLocation {
        directory,
        file_name,
        root_directory: &root.directory,
        display_directory: &root.display_directory,
        module_directories: &module_directories,
    };

    let mut modules = Vec::new();
    let includes = load::includes(
        &root_file,
        &location_of(paths::ROOT, root.file_name),
        sources,
        diagnostics,
    );
    modules.push(LoadedModule {
        directory: paths::ROOT.to_string(),
        is_root: true,
        file: root_file,
        includes,
    });
    for site in &sites {
        let directory = paths::on_disk(&root.directory, &site.directory);
        let display_directory = paths::on_disk(&root.display_directory, &site.directory);
        let Some(file) = load::read_definition(
            &directory.join(site.file_name),
            &display_directory.join(site.file_name),
            format_of(site.file_name)?,
            sources,
            diagnostics,
        ) else {
            continue;
        };
        let includes = load::includes(
            &file,
            &location_of(&site.directory, site.file_name),
            sources,
            diagnostics,
        );
        modules.push(LoadedModule {
            directory: site.directory.clone(),
            is_root: false,
            file,
            includes,
        });
    }

    let no_languages = BTreeSet::new();
    let languages = match &config.generate {
        Some(generate) => &generate.languages,
        None => &no_languages,
    };
    let lowered = lower::lower(&modules, sources, languages, diagnostics);
    validate::check_references(
        &Declared {
            tree: &lowered.tree,
            names: &lowered.declared,
        },
        diagnostics,
    );
    validate::check_retired_modules_are_gone(&lowered.tree, &config.retired, diagnostics);
    diagnostics.is_empty().then_some((lowered.tree, config))
}

/// The syntax a definition file's name says it is written in.
fn format_of(file_name: &'static str) -> Option<Format> {
    let format = Format::of(Path::new(file_name));
    debug_assert!(
        format.is_some(),
        "both definition file names carry a format's extension"
    );
    format
}

/// Everything this run generates. The manifest lists every other file, so it
/// is rendered last, over the rest of the tree.
fn generated_tree(
    tree: &Tree,
    retired: &Retired,
    generate: &GenerateConfig,
    manifest_path: &str,
    diagnostics: &mut Diagnostics,
) -> Option<GeneratedTree> {
    let output = generate.output.value.as_str();
    let mut generated = GeneratedTree::default();
    let rendered = render_what_is_asked_for(tree, generate, &mut generated).and_then(|()| {
        let files: Vec<&str> = generated.paths().collect();
        manifest::render(tree, retired, output, &files).map_err(RenderError::Json)
    });
    match rendered {
        Ok(manifest) => {
            generated.insert(manifest_path, manifest);
            Some(generated)
        }
        Err(error) => {
            diagnostics.unplaced(
                Code::Internal,
                format!(
                    "the generated tree could not be rendered: {error} — a defect in \
                     bc-telemetry-gen"
                ),
            );
            None
        }
    }
}

/// Every generated file but the manifest: what the root's `[generate]` table
/// asks for, and nothing it does not.
fn render_what_is_asked_for(
    tree: &Tree,
    generate: &GenerateConfig,
    generated: &mut GeneratedTree,
) -> Result<(), RenderError> {
    for dialect in &generate.dialects {
        match dialect {
            Dialect::Postgresql => {
                layout::render_postgresql(tree, &generate.output.value, generated)?;
            }
        }
    }
    if let Some(export) = &generate.grafana {
        grafana::render(tree, &generate.output.value, export, generated)?;
    }
    for language in &generate.languages {
        match language {
            Language::Cpp => cpp::render(&tree.modules, &generate.output.value, generated)
                .map_err(|_| RenderError::Defect("writing C++ into a string failed".to_string()))?,
            Language::Rust => rust::render(&tree.modules, &generate.output.value, generated)
                .map_err(|_| {
                    RenderError::Defect("writing Rust into a string failed".to_string())
                })?,
        }
    }
    Ok(())
}

/// The committed manifest, if there is one where this tree's `output` puts it.
/// One that is there and cannot be read is a finding, since the run cannot then
/// tell its own files from anyone else's, nor a change that breaks readers
/// from one that does not.
fn previous_manifest(
    root: &Root,
    manifest_path: &str,
    sources: &mut Sources,
    diagnostics: &mut Diagnostics,
) -> Result<Option<PreviousManifest>, Reported> {
    let display = root.display_directory.join(manifest_path);
    let bytes = match fs::read(root.directory.join(manifest_path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        // Anything else — the manifest is there and unreadable, or `output`
        // names something that is not a directory — is not an absent manifest:
        // reading it as one would let the run treat its own files as strangers.
        Err(error) => {
            diagnostics.unplaced(
                Code::Io,
                format!("cannot read `{}`: {error}", display.display()),
            );
            return Err(Reported);
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        diagnostics.unplaced(
            Code::ManifestUnreadable,
            format!("`{}` is not UTF-8 text", display.display()),
        );
        return Err(Reported);
    };
    let file = sources.add(display.clone(), text);
    let table = match front::parse(Format::Json, file, &sources.file(file).text) {
        Ok(table) => table,
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            return Err(Reported);
        }
    };
    let context = format!("the manifest `{}`", display.display());
    manifest::read(&table, &context, diagnostics)
        .map(Some)
        .ok_or(Reported)
}

fn report_difference(difference: &Difference, root: &Root, diagnostics: &mut Diagnostics) {
    let findings = [
        (
            Code::OutputChanged,
            &difference.changed,
            "differs from what the definitions generate",
        ),
        (
            Code::OutputMissing,
            &difference.missing,
            "is generated but missing",
        ),
        (
            Code::OutputStale,
            &difference.stale,
            "was generated by an earlier run and is no longer produced",
        ),
        (
            Code::OutputUnexpected,
            &difference.unexpected,
            "is in a directory the generator owns, and no run generated it",
        ),
    ];
    for (code, paths, why) in findings {
        for path in paths {
            diagnostics.unplaced(code, format!("`{}` {why}", root.display_of(path)));
        }
    }
}
