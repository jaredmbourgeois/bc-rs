//! Lowering parsed definition files into the model.
//!
//! Every file is walked with the strict walker, so an unknown key anywhere is a
//! finding. A module's own definition file is the one authority for its name and
//! `schema_version`; a file it includes repeats both, so it says what it is when
//! read alone, and a mismatch is a finding rather than an adoption. Items merge
//! across a module's files by name, and the same name declared twice is refused
//! wherever the two declarations live.
//!
//! A reference is resolved here: `metric.request_duration` written in module
//! `payments` lowers to the same thing as `metric.payments.request_duration`,
//! so the model — and the manifest rendered from it — never depends on which
//! way a definition spelled it. Whether what it names exists is the reference
//! check's question, asked once every module has been read.
//!
//! A declaration whose body has findings is left out of the model but its name
//! is still recorded as declared, so a reference to it is not refused a second
//! time as dangling — one mistake, one finding.

use std::collections::btree_map::Entry as MapEntry;
use std::collections::{BTreeMap, BTreeSet};

use bc_telemetry::{MetricKind, Severity, SpanKind};

use crate::adapters::{Language, NameRole, reserved_in};
use crate::diagnostics::{Code, Diagnostics, candidates_for, did_you_mean};
use crate::document::{Entry, Table, Value};
use crate::layout::queries::is_a_column_the_records_statement_adds;
use crate::layout::views::is_a_column_every_view_carries;
use crate::load::LoadedModule;
use crate::model::{
    Aggregation, Dashboard, ErrorClass, ErrorDeclaration, ItemKind, ItemRef, Items, Keyword,
    LogDeclaration, MAXIMUM_ERROR_CODE, MetricDeclaration, Module, Panel, PanelSource, Query,
    QueryRef, Render, ScalarType, SpanDeclaration, Tree, Unit,
};
use crate::names::{
    DASHBOARD_UID_CHARACTERS, IDENTIFIER_BYTES, NameShape, dashboard_uid, view_name,
};
use crate::source::{FileId, Sources, Span, Spanned};
use crate::walker::TableWalker;

/// The tables a definition file declares things in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeclarationKind {
    Span,
    Metric,
    Log,
    Error,
    Query,
    Dashboard,
}

impl DeclarationKind {
    const ALL: [Self; 6] = [
        Self::Span,
        Self::Metric,
        Self::Log,
        Self::Error,
        Self::Query,
        Self::Dashboard,
    ];

    /// The table key, which is also the noun a message uses.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Span => "span",
            Self::Metric => "metric",
            Self::Log => "log",
            Self::Error => "error",
            Self::Query => "query",
            Self::Dashboard => "dashboard",
        }
    }

    fn shape(self) -> NameShape {
        match self {
            Self::Span | Self::Metric | Self::Log | Self::Error => NameShape::Snake,
            Self::Query | Self::Dashboard => NameShape::Kebab,
        }
    }

    /// The item this declares, for a kind that declares one: a query and a
    /// dashboard consume the vocabulary and generate no language's code.
    fn item_kind(self) -> Option<ItemKind> {
        match self {
            Self::Span => Some(ItemKind::Span),
            Self::Metric => Some(ItemKind::Metric),
            Self::Log => Some(ItemKind::Log),
            Self::Error => Some(ItemKind::Error),
            Self::Query | Self::Dashboard => None,
        }
    }

    /// The other kind stored in the same table of records, whose names this
    /// kind's may therefore not repeat: a log and an error are both stored in
    /// `telemetry.logs`, by name.
    fn shares_records_with(self) -> Option<Self> {
        match self {
            Self::Log => Some(Self::Error),
            Self::Error => Some(Self::Log),
            Self::Span | Self::Metric | Self::Query | Self::Dashboard => None,
        }
    }
}

impl From<ItemKind> for DeclarationKind {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Span => Self::Span,
            ItemKind::Metric => Self::Metric,
            ItemKind::Log => Self::Log,
            ItemKind::Error => Self::Error,
        }
    }
}

/// Every name one module declared, whether or not its declaration lowered.
#[derive(Debug, Default)]
pub(crate) struct DeclaredNames {
    names: BTreeMap<DeclarationKind, BTreeSet<String>>,
}

impl DeclaredNames {
    pub(crate) fn contains(&self, kind: DeclarationKind, name: &str) -> bool {
        self.names
            .get(&kind)
            .is_some_and(|names| names.contains(name))
    }

    pub(crate) fn of(&self, kind: DeclarationKind) -> impl Iterator<Item = &str> {
        self.names
            .get(&kind)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

/// Everything lowering produced. A module whose header had findings is in
/// neither map; the run has failed then, and its declarations were walked only
/// so that one run reports every other finding.
#[derive(Debug, Default)]
pub(crate) struct Lowered {
    pub(crate) tree: Tree,
    /// Each module's declared names, by module name.
    pub(crate) declared: BTreeMap<String, DeclaredNames>,
}

const ITEM_KEYS: [&str; 6] = ["span", "metric", "log", "error", "query", "dashboard"];
const ROOT_KEYS: &[&str] = &[
    "module",
    "include",
    "generate",
    "retired",
    "span",
    "metric",
    "log",
    "error",
    "query",
    "dashboard",
];
const MODULE_KEYS: &[&str] = &[
    "module",
    "include",
    "span",
    "metric",
    "log",
    "error",
    "query",
    "dashboard",
];
const INCLUDED_KEYS: &[&str] = &[
    "module",
    "span",
    "metric",
    "log",
    "error",
    "query",
    "dashboard",
];

/// The keys only the root file may carry.
const ROOT_ONLY_KEYS: [&str; 2] = ["generate", "retired"];

pub(crate) fn lower(
    modules: &[LoadedModule],
    sources: &Sources,
    languages: &BTreeSet<Language>,
    diagnostics: &mut Diagnostics,
) -> Lowered {
    let mut lowered = Lowered::default();
    // Where each module name was first declared, for the finding that refuses a
    // second.
    let mut first_named = BTreeMap::new();
    for module in modules {
        lower_module(
            module,
            sources,
            languages,
            &mut first_named,
            &mut lowered,
            diagnostics,
        );
    }
    lowered
}

fn lower_module(
    loaded: &LoadedModule,
    sources: &Sources,
    languages: &BTreeSet<Language>,
    first_named: &mut BTreeMap<String, Span>,
    lowered: &mut Lowered,
    diagnostics: &mut Diagnostics,
) {
    let mut walker = TableWalker::new(
        &loaded.file.table,
        file_context(sources, loaded.file.file),
        diagnostics,
    );
    // A root may configure the build without being a module itself. Any
    // other file — and a root that declares or includes anything — is a module
    // and says so.
    let is_only_a_build_root = loaded.is_root && configures_the_build_only(&loaded.file.table);
    let header = if is_only_a_build_root {
        None
    } else {
        module_header(&mut walker)
    };
    // The loader read `include`, and the root reader `[generate]` and
    // `[retired]`; each reported any problem with what it read.
    walker.consume("include");
    if loaded.is_root {
        for key in ROOT_ONLY_KEYS {
            walker.consume(key);
        }
    } else {
        refuse_root_only_keys(
            &mut walker,
            "a module's file carries `[module]`, `include` and declarations",
        );
    }

    // A file with no usable header has no name to qualify its references with;
    // they resolve to the empty name, which nothing checks, since the module
    // never joins the tree.
    let module_name = match &header {
        Some(header) => header.name.value.clone(),
        None => String::new(),
    };
    let mut declarations = Declarations::of_module(module_name, languages.clone());
    declarations.walk(&mut walker, sources);
    walker.finish(if loaded.is_root {
        ROOT_KEYS
    } else {
        MODULE_KEYS
    });

    for included in &loaded.includes {
        let mut walker = TableWalker::new(
            &included.table,
            file_context(sources, included.file),
            diagnostics,
        );
        let included_header = module_header(&mut walker);
        if let (Some(header), Some(included_header)) = (&header, &included_header) {
            corroborate(header, included_header, walker.diagnostics());
        }
        if let Some(entry) = walker.take("include") {
            walker.diagnostics().at(
                Code::InvalidInclude,
                entry.key_span,
                "`include` belongs to a module's own definition file; an included file carries \
                 `[module]` and declarations",
            );
        }
        refuse_root_only_keys(
            &mut walker,
            "an included file carries `[module]` and declarations",
        );
        declarations.walk(&mut walker, sources);
        walker.finish(INCLUDED_KEYS);
    }

    let Some(header) = header else {
        return;
    };
    match first_named.entry(header.name.value.clone()) {
        MapEntry::Occupied(first) => {
            diagnostics.at(
                Code::DuplicateModule,
                header.name.span,
                format!(
                    "module `{}` is declared twice; first at {} — a module's name is unique \
                     across the tree",
                    header.name.value,
                    sources.describe(*first.get())
                ),
            );
            return;
        }
        MapEntry::Vacant(vacant) => {
            vacant.insert(header.name.span);
        }
    }
    lowered
        .declared
        .insert(header.name.value.clone(), declarations.declared);
    lowered.tree.modules.insert(
        header.name.value,
        Module {
            directory: loaded.directory.clone(),
            name_span: header.name.span,
            schema_version: header.schema_version,
            items: declarations.items,
        },
    );
}

/// Whether the root file only configures the build: it carries `[generate]` and
/// neither a module header, nor an include, nor a declaration.
fn configures_the_build_only(root: &Table) -> bool {
    root.get("generate").is_some()
        && root.get("module").is_none()
        && root.get("include").is_none()
        && ITEM_KEYS.iter().all(|key| root.get(key).is_none())
}

fn refuse_root_only_keys(walker: &mut TableWalker<'_, '_>, what_this_file_carries: &str) {
    for key in ROOT_ONLY_KEYS {
        if let Some(entry) = walker.take(key) {
            let message = format!(
                "`{key}` belongs to the root definition file only; {what_this_file_carries}"
            );
            walker
                .diagnostics()
                .at(Code::RootOnly, entry.key_span, message);
        }
    }
}

fn file_context(sources: &Sources, file: FileId) -> String {
    format!(
        "the definition file `{}`",
        sources.display_path(file).display()
    )
}

/// A file's `[module]` header: the module's name and `schema_version`.
struct Header {
    name: Spanned<String>,
    schema_version: Spanned<u32>,
}

fn module_header(walker: &mut TableWalker<'_, '_>) -> Option<Header> {
    let table = walker.required_table("module")?;
    let mut module = TableWalker::new(table, "`[module]`", walker.diagnostics());
    let name = module.required_string("name");
    let version = module.required_integer("schema_version");
    module.finish(&["name", "schema_version"]);

    let diagnostics = walker.diagnostics();
    let name = name.filter(|name| {
        let admitted = NameShape::Snake.admits(&name.value);
        if !admitted {
            diagnostics.at(
                Code::InvalidName,
                name.span,
                format!(
                    "module name `{}` is not {}",
                    name.value,
                    NameShape::Snake.describe()
                ),
            );
        }
        admitted
    });
    // Every record stores its module's version in the layout's `integer`
    // `schema_version` column, so a version is at most that column's largest
    // value.
    let schema_version = version.and_then(|version| {
        let in_range = i32::try_from(version.value)
            .ok()
            .filter(|number| *number >= 1)
            .and_then(|number| u32::try_from(number).ok());
        if in_range.is_none() {
            diagnostics.at(
                Code::InvalidValue,
                version.span,
                format!(
                    "`schema_version` must be a whole number from 1 to {}; got {}",
                    i32::MAX,
                    version.value
                ),
            );
        }
        in_range.map(|value| Spanned {
            value,
            span: version.span,
        })
    });
    Some(Header {
        name: name?,
        schema_version: schema_version?,
    })
}

/// An included file repeats its module's name and version; it never sets its
/// own, since a module's version is declared once, in its own file.
fn corroborate(module: &Header, included: &Header, diagnostics: &mut Diagnostics) {
    if included.name.value != module.name.value {
        diagnostics.at(
            Code::ModuleMismatch,
            included.name.span,
            format!(
                "this file belongs to module `{}`, but the file that includes it is module `{}`",
                included.name.value, module.name.value
            ),
        );
    }
    if included.schema_version.value != module.schema_version.value {
        diagnostics.at(
            Code::VersionMismatch,
            included.schema_version.span,
            format!(
                "`schema_version` is {} here, but the module declares {}; an included file \
                 repeats its module's version and never sets its own",
                included.schema_version.value, module.schema_version.value
            ),
        );
    }
}

/// One module's items being merged across its files, and which names each kind
/// has used.
struct Declarations {
    /// The module's name, which an unqualified reference resolves to.
    module: String,
    items: Items,
    declared: DeclaredNames,
    first_declared: BTreeMap<(DeclarationKind, String), Span>,
    /// The error that first declared each code, and where.
    first_coded: BTreeMap<u32, (String, Span)>,
    /// The languages `[generate]` names, whose reserved words no name may be.
    languages: BTreeSet<Language>,
}

impl Declarations {
    fn of_module(module: String, languages: BTreeSet<Language>) -> Self {
        Self {
            module,
            languages,
            items: Items::default(),
            declared: DeclaredNames::default(),
            first_declared: BTreeMap::new(),
            first_coded: BTreeMap::new(),
        }
    }

    fn walk(&mut self, walker: &mut TableWalker<'_, '_>, sources: &Sources) {
        for kind in DeclarationKind::ALL {
            let Some(table) = walker.optional_table(kind.key()) else {
                continue;
            };
            for entry in &table.entries {
                self.declare(kind, entry, sources, walker.diagnostics());
            }
        }
    }

    fn declare(
        &mut self,
        kind: DeclarationKind,
        entry: &Entry,
        sources: &Sources,
        diagnostics: &mut Diagnostics,
    ) {
        let noun = kind.key();
        let name = &entry.key;
        if !kind.shape().admits(name) {
            diagnostics.at(
                Code::InvalidName,
                entry.key_span,
                format!("{noun} name `{name}` is not {}", kind.shape().describe()),
            );
        }
        match self.first_declared.entry((kind, name.clone())) {
            MapEntry::Occupied(first) => {
                diagnostics.at(
                    Code::DuplicateItem,
                    entry.key_span,
                    format!(
                        "{noun} `{name}` is declared twice; first at {}",
                        sources.describe(*first.get())
                    ),
                );
                return;
            }
            MapEntry::Vacant(vacant) => {
                vacant.insert(entry.key_span);
            }
        }
        self.declared
            .names
            .entry(kind)
            .or_default()
            .insert(name.clone());
        if let Some(twin) = kind.shares_records_with() {
            if let Some(first) = self.first_declared.get(&(twin, name.clone())) {
                diagnostics.at(
                    Code::DuplicateItem,
                    entry.key_span,
                    format!(
                        "{noun} `{name}` shares its name with {} `{name}`, declared at {}; both \
                         are stored as log records, so a module gives one name to one of them",
                        twin.key(),
                        sources.describe(*first)
                    ),
                );
                return;
            }
        }
        self.hold_to_engine_limits(kind, entry, diagnostics);
        if let Some(item_kind) = kind.item_kind() {
            refuse_a_reserved_word(
                &self.languages,
                NameRole::Item(item_kind),
                &entry.key,
                entry.key_span,
                &format!("{noun} `{name}`"),
                diagnostics,
            );
        }

        let Value::Table(body) = &entry.value.value else {
            diagnostics.at(
                Code::TypeMismatch,
                entry.value.span,
                format!(
                    "{noun} `{name}` must be a table, got {}",
                    entry.value.value.describe()
                ),
            );
            return;
        };
        let context = format!("{noun} `{name}`");
        self.lower_body(
            kind,
            name.clone(),
            entry,
            body,
            context,
            sources,
            diagnostics,
        );
    }

    /// Lowers one declaration's table into the module's items.
    #[expect(
        clippy::too_many_arguments,
        reason = "what one declaration is lowered from: its kind, name, entry and table, the \
                  context a finding names it by, and the run's sources and sink"
    )]
    fn lower_body(
        &mut self,
        kind: DeclarationKind,
        name: String,
        entry: &Entry,
        body: &Table,
        context: String,
        sources: &Sources,
        diagnostics: &mut Diagnostics,
    ) {
        match kind {
            DeclarationKind::Span => {
                if let Some(declaration) =
                    span_declaration(body, context, &self.languages, diagnostics)
                {
                    self.items.spans.insert(name, declaration);
                }
            }
            DeclarationKind::Metric => {
                if let Some(declaration) =
                    metric_declaration(body, context, &self.languages, diagnostics)
                {
                    self.items.metrics.insert(name, declaration);
                }
            }
            DeclarationKind::Log => {
                if let Some(declaration) =
                    log_declaration(body, context, &self.languages, diagnostics)
                {
                    self.items.logs.insert(name, declaration);
                }
            }
            DeclarationKind::Error => {
                if let Some(declaration) =
                    error_declaration(body, context, &self.languages, diagnostics)
                {
                    self.hold_to_one_error_per_code(&name, &declaration, sources, diagnostics);
                    // A refusal is never reported, so it has no view to name.
                    if declaration.class == ErrorClass::Failure {
                        self.hold_view_name_to_its_limit(kind, entry, diagnostics);
                    }
                    self.items.errors.insert(name, declaration);
                }
            }
            DeclarationKind::Query => {
                if let Some(query) = query(body, context, &self.module, diagnostics) {
                    self.items.queries.insert(name, query);
                }
            }
            DeclarationKind::Dashboard => {
                if let Some(dashboard) = dashboard(body, context, &self.module, diagnostics) {
                    self.items.dashboards.insert(name, dashboard);
                }
            }
        }
    }

    /// The names derived from a declaration are held to the limit of the engine
    /// that carries them: a view's name to PostgreSQL's 63 bytes, past which it
    /// would truncate silently into a collision, and a dashboard's uid to
    /// Grafana's 40 characters. An error's is held once its class is known.
    fn hold_to_engine_limits(
        &self,
        kind: DeclarationKind,
        entry: &Entry,
        diagnostics: &mut Diagnostics,
    ) {
        match kind {
            DeclarationKind::Span | DeclarationKind::Metric | DeclarationKind::Log => {
                self.hold_view_name_to_its_limit(kind, entry, diagnostics);
            }
            DeclarationKind::Dashboard => self.hold_uid_to_its_limit(entry, diagnostics),
            DeclarationKind::Error | DeclarationKind::Query => {}
        }
    }

    fn hold_view_name_to_its_limit(
        &self,
        kind: DeclarationKind,
        entry: &Entry,
        diagnostics: &mut Diagnostics,
    ) {
        // A module with no usable header has no name to derive one from.
        if self.module.is_empty() {
            return;
        }
        let name = &entry.key;
        let view = view_name(kind.key(), &self.module, name);
        if view.len() > IDENTIFIER_BYTES {
            diagnostics.at(
                Code::NameTooLong,
                entry.key_span,
                format!(
                    "{} `{name}` in module `{}` has the view name `{view}`, which is {} bytes; \
                     PostgreSQL keeps {IDENTIFIER_BYTES} and would cut it into another view's \
                     name — shorten the module's name or the item's",
                    kind.key(),
                    self.module,
                    view.len()
                ),
            );
        }
    }

    fn hold_uid_to_its_limit(&self, entry: &Entry, diagnostics: &mut Diagnostics) {
        if self.module.is_empty() {
            return;
        }
        let name = &entry.key;
        let uid = dashboard_uid(&self.module, name);
        let characters = uid.chars().count();
        if characters > DASHBOARD_UID_CHARACTERS {
            diagnostics.at(
                Code::NameTooLong,
                entry.key_span,
                format!(
                    "dashboard `{name}` in module `{}` has the Grafana uid `{uid}`, which is \
                     {characters} characters; Grafana takes {DASHBOARD_UID_CHARACTERS} — \
                     shorten the module's name or the dashboard's",
                    self.module
                ),
            );
        }
    }

    fn hold_to_one_error_per_code(
        &mut self,
        name: &str,
        declaration: &ErrorDeclaration,
        sources: &Sources,
        diagnostics: &mut Diagnostics,
    ) {
        match self.first_coded.entry(declaration.code.value) {
            MapEntry::Occupied(first) => {
                let (first_name, first_span) = first.get();
                diagnostics.at(
                    Code::OneCodeTwoErrors,
                    declaration.code.span,
                    format!(
                        "error `{name}` declares code {}, which error `{first_name}` declares at \
                         {}; a code is one error's identity",
                        declaration.code.value,
                        sources.describe(*first_span)
                    ),
                );
            }
            MapEntry::Vacant(vacant) => {
                vacant.insert((name.to_string(), declaration.code.span));
            }
        }
    }
}

fn span_declaration(
    body: &Table,
    context: String,
    languages: &BTreeSet<Language>,
    diagnostics: &mut Diagnostics,
) -> Option<SpanDeclaration> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let description = walker.required_text("description");
    // A span that says nothing is the trace's own work, as OpenTelemetry's
    // default kind is; this is the one place that default is written.
    let kind = if walker.has("kind") {
        walker
            .required_keyword("kind", &SpanKind::keywords(), SpanKind::from_keyword)
            .map(|kind| kind.value)
    } else {
        Some(SpanKind::Internal)
    };
    let attributes = scalar_map(
        &mut walker,
        "attributes",
        "attribute",
        ItemKind::Span,
        languages,
    );
    let events = name_list(&mut walker, "events", "event", NameShape::Snake);
    // A reserved event name is refused where it is written; the run then fails
    // on that finding, so the span is lowered as declared.
    if let Some(events) = &events {
        let context = format!("`events` on {}", walker.context());
        for event in events {
            refuse_a_reserved_word(
                languages,
                NameRole::Event,
                &event.value,
                event.span,
                &format!("event `{}` in {context}", event.value),
                walker.diagnostics(),
            );
        }
    }
    walker.finish(&["description", "kind", "attributes", "events"]);
    Some(SpanDeclaration {
        description: description?.value,
        kind: kind?,
        attributes: attributes?,
        events: events?.into_iter().map(|event| event.value).collect(),
    })
}

fn metric_declaration(
    body: &Table,
    context: String,
    languages: &BTreeSet<Language>,
    diagnostics: &mut Diagnostics,
) -> Option<MetricDeclaration> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let description = walker.required_text("description");
    let kind = walker.required_keyword("type", &MetricKind::keywords(), MetricKind::from_keyword);
    let unit = walker.required_keyword("unit", &Unit::keywords(), Unit::from_keyword);
    let attributes = scalar_map(
        &mut walker,
        "attributes",
        "attribute",
        ItemKind::Metric,
        languages,
    );
    walker.finish(&["description", "type", "unit", "attributes"]);
    Some(MetricDeclaration {
        description: description?.value,
        kind: kind?.value,
        unit: unit?.value,
        attributes: attributes?,
    })
}

fn log_declaration(
    body: &Table,
    context: String,
    languages: &BTreeSet<Language>,
    diagnostics: &mut Diagnostics,
) -> Option<LogDeclaration> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let description = walker.required_text("description");
    let severity = walker.required_keyword("level", &Severity::keywords(), Severity::from_keyword);
    let fields = scalar_map(&mut walker, "fields", "field", ItemKind::Log, languages);
    walker.finish(&["description", "level", "fields"]);
    Some(LogDeclaration {
        description: description?.value,
        severity: severity?.value,
        fields: fields?,
    })
}

fn error_declaration(
    body: &Table,
    context: String,
    languages: &BTreeSet<Language>,
    diagnostics: &mut Diagnostics,
) -> Option<ErrorDeclaration> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let description = walker.required_text("description");
    let class = walker.required_keyword("class", &ErrorClass::keywords(), ErrorClass::from_keyword);
    let code = walker.required_integer("code");
    let fields = scalar_map(&mut walker, "fields", "field", ItemKind::Error, languages);
    let context = walker.context().to_string();
    walker.finish(&["description", "class", "code", "fields"]);

    let code = code.and_then(|code| {
        let in_range = u32::try_from(code.value)
            .ok()
            .filter(|number| *number <= MAXIMUM_ERROR_CODE);
        if in_range.is_none() {
            diagnostics.at(
                Code::InvalidValue,
                code.span,
                format!(
                    "`code` on {context} must be a whole number from 0 to {MAXIMUM_ERROR_CODE}; \
                     got {}",
                    code.value
                ),
            );
        }
        in_range.map(|value| Spanned {
            value,
            span: code.span,
        })
    });
    Some(ErrorDeclaration {
        description: description?.value,
        class: class?.value,
        code: code?,
        fields: fields?,
    })
}

fn query(
    body: &Table,
    context: String,
    module: &str,
    diagnostics: &mut Diagnostics,
) -> Option<Query> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let description = walker.required_text("description");
    let measure = walker.required_string("measure");
    let aggregation = walker.required_keyword(
        "aggregation",
        &Aggregation::keywords(),
        Aggregation::from_keyword,
    );
    let window = walker.required_string("window");
    let group_by = name_list(&mut walker, "group_by", "attribute", NameShape::Snake);
    let context = walker.context().to_string();
    walker.finish(&[
        "description",
        "measure",
        "aggregation",
        "window",
        "group_by",
    ]);

    let measure = measure.and_then(|measure| {
        let parsed = item_ref(&measure.value, module);
        if parsed.is_none() {
            diagnostics.at(
                Code::InvalidValue,
                measure.span,
                format!(
                    "`measure` on {context} must name a declared item as `<kind>.<name>`, or as \
                     `<kind>.<module>.<name>` in another module, where the kind is {}; got `{}`",
                    ItemKind::keywords().join(", "),
                    measure.value
                ),
            );
        }
        parsed.map(|value| Spanned {
            value,
            span: measure.span,
        })
    });
    let window_seconds = window.and_then(|window| window_seconds(&window, &context, diagnostics));
    Some(Query {
        description: description?.value,
        measure: measure?,
        aggregation: aggregation?,
        window_seconds: window_seconds?,
        group_by: group_by?,
    })
}

fn dashboard(
    body: &Table,
    context: String,
    module: &str,
    diagnostics: &mut Diagnostics,
) -> Option<Dashboard> {
    let mut walker = TableWalker::new(body, context, diagnostics);
    let title = walker.required_text("title");
    let elements = walker.required_array("panel");
    let dashboard_context = walker.context().to_string();
    walker.finish(&["title", "panel"]);

    let (elements, array_span) = elements?;
    if elements.is_empty() {
        diagnostics.at(
            Code::InvalidValue,
            array_span,
            format!("{dashboard_context} has no panels; a dashboard shows at least one"),
        );
        return None;
    }
    let mut panels = Vec::new();
    let mut every_panel_lowered = true;
    for (index, element) in elements.iter().enumerate() {
        let context = format!("panel {} of {dashboard_context}", index + 1);
        let Value::Table(table) = &element.value else {
            diagnostics.at(
                Code::TypeMismatch,
                element.span,
                format!(
                    "{context} must be a table, got {}",
                    element.value.describe()
                ),
            );
            every_panel_lowered = false;
            continue;
        };
        match panel(table, context, module, diagnostics) {
            Some(panel) => panels.push(panel),
            None => every_panel_lowered = false,
        }
    }
    Some(Dashboard {
        title: title?.value,
        panels: every_panel_lowered.then_some(panels)?,
    })
}

fn panel(
    table: &Table,
    context: String,
    module: &str,
    diagnostics: &mut Diagnostics,
) -> Option<Panel> {
    let mut walker = TableWalker::new(table, context, diagnostics);
    let title = walker.required_text("title");
    let source = walker.required_string("query");
    let render = walker.required_keyword("render", &Render::keywords(), Render::from_keyword);
    let context = walker.context().to_string();
    walker.finish(&["title", "query", "render"]);

    let source = source.and_then(|source| {
        let parsed = panel_source(&source.value, module);
        if parsed.is_none() {
            diagnostics.at(
                Code::InvalidValue,
                source.span,
                format!(
                    "`query` on {context} must reference `query.<name>`, or `log.<name>` or \
                     `error.<name>` for a logs panel — each as `<kind>.<module>.<name>` in \
                     another module; got `{}`",
                    source.value
                ),
            );
        }
        parsed.map(|value| Spanned {
            value,
            span: source.span,
        })
    });
    Some(Panel {
        title: title?.value,
        source: source?,
        render: render?,
    })
}

/// A reference's module and name, from what follows its kind: `<name>` in
/// `module`, or `<module>.<name>`. `None` for any other shape, or an empty part.
fn module_and_name(after_the_kind: &str, module: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = after_the_kind.split('.').collect();
    let (module, name) = match parts.as_slice() {
        [name] => (module, *name),
        [other_module, name] if !other_module.is_empty() => (*other_module, *name),
        _ => return None,
    };
    if name.is_empty() {
        return None;
    }
    Some((module.to_string(), name.to_string()))
}

/// A reference to a declared item, written in `module`: `<kind>.<name>` or
/// `<kind>.<module>.<name>`.
fn item_ref(text: &str, module: &str) -> Option<ItemRef> {
    let (kind, rest) = text.split_once('.')?;
    let kind = ItemKind::from_keyword(kind)?;
    let (module, name) = module_and_name(rest, module)?;
    Some(ItemRef { kind, module, name })
}

/// What a panel shows: a query, or the records of a log or an error.
fn panel_source(text: &str, module: &str) -> Option<PanelSource> {
    if let Some(rest) = text.strip_prefix("query.") {
        let (module, name) = module_and_name(rest, module)?;
        return Some(PanelSource::Query(QueryRef { module, name }));
    }
    let item = item_ref(text, module)?;
    matches!(item.kind, ItemKind::Log | ItemKind::Error).then_some(PanelSource::Records(item))
}

/// A window such as `5m`: a positive whole number of seconds, minutes, hours
/// or days.
fn window_seconds(
    window: &Spanned<String>,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<u64> {
    let text = window.value.as_str();
    let unit_seconds = match text.chars().last() {
        Some('s') => Some(1),
        Some('m') => Some(60),
        Some('h') => Some(3_600),
        Some('d') => Some(86_400),
        _ => None,
    };
    let count = text
        .get(..text.len().saturating_sub(1))
        .filter(|digits| !digits.is_empty() && digits.chars().all(|digit| digit.is_ascii_digit()))
        .and_then(|digits| digits.parse::<u64>().ok());
    let seconds = unit_seconds
        .zip(count)
        .and_then(|(unit, count)| count.checked_mul(unit))
        .filter(|seconds| *seconds > 0);
    if seconds.is_none() {
        diagnostics.at(
            Code::InvalidValue,
            window.span,
            format!(
                "`window` on {context} must be a positive whole number of `s`, `m`, `h` or `d`, \
                 like `5m`; got `{text}`"
            ),
        );
    }
    seconds
}

/// Refuses `name` where a language `[generate]` names cannot use it in `role`:
/// the consumer's compiler would refuse it later, further from the
/// definition. Says whether the name was admitted.
fn refuse_a_reserved_word(
    languages: &BTreeSet<Language>,
    role: NameRole,
    name: &str,
    span: Span,
    what: &str,
    diagnostics: &mut Diagnostics,
) -> bool {
    let Some(language) = reserved_in(languages, role, name) else {
        return true;
    };
    diagnostics.at(
        Code::ReservedName,
        span,
        format!(
            "{what} cannot be named so in {}, which `languages` generates; pick another name",
            language.display_name()
        ),
    );
    false
}

/// Refuses `name`, an attribute or field of an item of `kind`, where the item's
/// typed view could not carry it as a column of its own. `what` names it as a
/// finding does. Says whether the name was admitted.
fn refuse_a_name_its_view_cannot_carry(
    kind: ItemKind,
    name: &str,
    span: Span,
    what: &str,
    diagnostics: &mut Diagnostics,
) -> bool {
    let mut admitted = true;
    // The name is a column of the item's view, and PostgreSQL keeps only the
    // first 63 bytes of one.
    if name.len() > IDENTIFIER_BYTES {
        diagnostics.at(
            Code::NameTooLong,
            span,
            format!(
                "{what} is {} bytes; PostgreSQL keeps {IDENTIFIER_BYTES} of a column's name and \
                 would cut it into another's — shorten it",
                name.len()
            ),
        );
        admitted = false;
    }
    // The view carries a column per attribute beside the columns every view of
    // its kind has, and one name cannot be two columns.
    if is_a_column_every_view_carries(kind, name) {
        diagnostics.at(
            Code::ReservedName,
            span,
            format!(
                "{what} has the name of a column every {} view carries; its own view could not \
                 hold both, so pick another name",
                kind.keyword()
            ),
        );
        admitted = false;
    }
    // And a log's or an error's records statement adds a column its view does
    // not carry: the severity, named as Grafana's logs panel reads it.
    if is_a_column_the_records_statement_adds(kind, name) {
        diagnostics.at(
            Code::ReservedName,
            span,
            format!(
                "{what} has the name its records statement gives the severity, which Grafana's \
                 logs panel reads it by; pick another name"
            ),
        );
        admitted = false;
    }
    admitted
}

/// A table of `name = "<type>"` entries: the attributes of an item of `kind`,
/// or a log's or an error's fields. Absent is empty; any finding in it leaves
/// the whole map out.
fn scalar_map(
    walker: &mut TableWalker<'_, '_>,
    key: &str,
    noun: &str,
    kind: ItemKind,
    languages: &BTreeSet<Language>,
) -> Option<BTreeMap<String, ScalarType>> {
    if !walker.has(key) {
        return Some(BTreeMap::new());
    }
    let table = walker.optional_table(key)?;
    let context = format!("`{key}` on {}", walker.context());
    let diagnostics = walker.diagnostics();
    let mut map = BTreeMap::new();
    let mut every_entry_lowered = true;
    for entry in &table.entries {
        if !NameShape::Snake.admits(&entry.key) {
            diagnostics.at(
                Code::InvalidName,
                entry.key_span,
                format!(
                    "{noun} name `{}` in {context} is not {}",
                    entry.key,
                    NameShape::Snake.describe()
                ),
            );
            every_entry_lowered = false;
        }
        let what = format!("{noun} `{}` in {context}", entry.key);
        every_entry_lowered &= refuse_a_reserved_word(
            languages,
            NameRole::Attribute(kind),
            &entry.key,
            entry.key_span,
            &what,
            diagnostics,
        );
        every_entry_lowered &= refuse_a_name_its_view_cannot_carry(
            kind,
            &entry.key,
            entry.key_span,
            &what,
            diagnostics,
        );
        match &entry.value.value {
            Value::String(text) => {
                if let Some(scalar) = ScalarType::from_keyword(text) {
                    map.insert(entry.key.clone(), scalar);
                } else {
                    let keywords = ScalarType::keywords();
                    diagnostics.at(
                        Code::InvalidValue,
                        entry.value.span,
                        format!(
                            "{noun} `{}` in {context} must be typed one of {}; got `{text}`{}",
                            entry.key,
                            keywords.join(" | "),
                            did_you_mean(&candidates_for(text, keywords.iter().copied()))
                        ),
                    );
                    every_entry_lowered = false;
                }
            }
            other => {
                diagnostics.at(
                    Code::TypeMismatch,
                    entry.value.span,
                    format!(
                        "{noun} `{}` in {context} must name its type as a string, got {}",
                        entry.key,
                        other.describe()
                    ),
                );
                every_entry_lowered = false;
            }
        }
    }
    every_entry_lowered.then_some(map)
}

/// A list of names — a span's events, a query's `group_by` — each held to
/// `shape` and none repeated. Absent is empty; any finding leaves it out.
fn name_list(
    walker: &mut TableWalker<'_, '_>,
    key: &str,
    noun: &str,
    shape: NameShape,
) -> Option<Vec<Spanned<String>>> {
    if !walker.has(key) {
        return Some(Vec::new());
    }
    let names = walker.optional_string_array(key)?;
    let context = format!("`{key}` on {}", walker.context());
    let diagnostics = walker.diagnostics();
    let mut every_name_admitted = true;
    let mut seen = BTreeSet::new();
    for name in &names {
        if !shape.admits(&name.value) {
            diagnostics.at(
                Code::InvalidName,
                name.span,
                format!(
                    "{noun} name `{}` in {context} is not {}",
                    name.value,
                    shape.describe()
                ),
            );
            every_name_admitted = false;
        }
        if !seen.insert(name.value.as_str()) {
            diagnostics.at(
                Code::DuplicateEntry,
                name.span,
                format!("{noun} `{}` appears twice in {context}", name.value),
            );
            every_name_admitted = false;
        }
    }
    every_name_admitted.then_some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Sources;

    fn window(text: &str) -> Option<u64> {
        let mut sources = Sources::default();
        let file = sources.add("w.toml".into(), String::new());
        let spanned = Spanned {
            value: text.to_string(),
            span: Span::start_of(file),
        };
        let mut diagnostics = Diagnostics::default();
        let seconds = window_seconds(&spanned, "query `q`", &mut diagnostics);
        assert_eq!(
            seconds.is_none(),
            !diagnostics.is_empty(),
            "`{text}`: a refusal is reported"
        );
        seconds
    }

    #[test]
    fn a_window_is_a_positive_count_of_one_unit() {
        assert_eq!(window("300s"), Some(300));
        assert_eq!(window("5m"), Some(300));
        assert_eq!(window("2h"), Some(7_200));
        assert_eq!(window("1d"), Some(86_400));
    }

    #[test]
    fn anything_else_is_refused() {
        // `+5m` is refused although Rust's integer parser takes a leading `+`.
        for text in [
            "",
            "m",
            "5",
            "0m",
            "+5m",
            "5 m",
            "5M",
            "1.5h",
            "99999999999999999999d",
        ] {
            assert_eq!(window(text), None, "`{text}`");
        }
    }

    fn item(kind: ItemKind, module: &str, name: &str) -> ItemRef {
        ItemRef {
            kind,
            module: module.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn a_reference_resolves_to_the_module_that_wrote_it_unless_it_names_another() {
        assert_eq!(
            item_ref("metric.request_duration", "payments"),
            Some(item(ItemKind::Metric, "payments", "request_duration"))
        );
        assert_eq!(
            item_ref("error.ledger.account_closed", "payments"),
            Some(item(ItemKind::Error, "ledger", "account_closed"))
        );
        // Naming its own module is the same reference.
        assert_eq!(
            item_ref("span.payments.authorize", "payments"),
            item_ref("span.authorize", "payments")
        );
    }

    #[test]
    fn a_reference_of_any_other_shape_is_not_one() {
        for text in [
            "request_duration",
            "gauge.cart_size",
            "metric.",
            "metric..cart_size",
            "metric.a.b.c",
            "metric.payments.",
            ".cart_size",
        ] {
            assert_eq!(item_ref(text, "payments"), None, "`{text}`");
        }
    }

    #[test]
    fn a_panel_shows_a_query_or_the_records_of_a_log_or_an_error() {
        assert_eq!(
            panel_source("query.checkout-p95", "store"),
            Some(PanelSource::Query(QueryRef {
                module: "store".to_string(),
                name: "checkout-p95".to_string()
            }))
        );
        assert_eq!(
            panel_source("query.payments.settlement-p90", "store"),
            Some(PanelSource::Query(QueryRef {
                module: "payments".to_string(),
                name: "settlement-p90".to_string()
            }))
        );
        assert_eq!(
            panel_source("error.payments.declined", "store"),
            Some(PanelSource::Records(item(
                ItemKind::Error,
                "payments",
                "declined"
            )))
        );
        // A span or a metric is aggregated by a query, never shown raw.
        assert_eq!(panel_source("span.checkout", "store"), None);
        assert_eq!(panel_source("metric.cart_size", "store"), None);
        assert_eq!(panel_source("query.a.b.c", "store"), None);
    }
}
