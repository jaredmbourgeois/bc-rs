//! The UniFFI binding: this crate's entire foreign contract, in one file.
//!
//! Present only under the `uniffi` feature. Every bound crate in the family
//! splits its binding into three layers — values cross verbatim, closures stay
//! the Rust-facing form, and foreign implementors get a trait that collapses
//! into the same closure. The two that need spelling here are the value layer
//! and the foreign-adapter layer.
//!
//! # Why the value layer is mirrored rather than exported in place
//!
//! [`MetadataValue`] cannot cross as it stands, for two reasons that are both
//! deliberate parts of its Rust design. It carries a [`MetadataDictionary`] — a
//! `BTreeMap`, which UniFFI has no form for — so that map crosses as a
//! **sorted sequence of key/value records** rather than being silently
//! rehashed, because `stringify` and `format_message` depend on its ordering.
//! And its `Error` arm holds a `Box<AnyError>`, whose payload is private and
//! size-pinned.
//!
//! # Why the mirror is a flat stream rather than a tree
//!
//! [`MetadataValue`] is recursive, and the obvious mirror — an enum with an
//! `Array { values: Vec<Self> }` arm — is recursive too. That shape cannot be
//! made safe at this boundary, and the reason is worth stating because it is not
//! obvious: **UniFFI's generated lift recurses before any code in this file
//! runs.** A foreign caller hands over a buffer; the scaffolding's `try_read`
//! walks it, calling itself once per level, and only then calls the function.
//! Dropping the lifted tree recurses again on the way out. A depth check placed
//! in an exported function sits *between* those two recursions and stops
//! neither, so a deeply nested value overflows the stack — a `SIGSEGV`, which
//! Rust does not unwind and no caller can catch.
//!
//! So the type is not recursive. A value crosses as [`ForeignMetadata`]: a flat
//! `Vec` of [`ForeignMetadataNode`] in **pre-order**, where a container node is
//! followed by exactly its declared children. Lifting that is a loop over a
//! vector, and so is dropping it.
//!
//! A stream rather than an index arena, deliberately. An arena admits
//! out-of-range indices and reference cycles, each needing its own detection
//! pass. Pre-order admits neither: the only way to be wrong is to miscount, and
//! one left-to-right pass ([`validate`]) catches that along with over-deep
//! nesting. Once a stream validates, its depth is known to be at most
//! [`MAX_METADATA_DEPTH`], so rebuilding the recursive Rust value is bounded and
//! safe — the ceiling means what it says, which is the whole point of the shape.
//!
//! Assembling a stream by hand is worse than assembling a tree, so the
//! `metadata_*` constructors below do it: each returns a complete
//! [`ForeignMetadata`], and composing them composes streams. A foreign author
//! writes nested calls and never sees a node index.
//!
//! [`AnyError`] crosses as a **record, not an object**. An error is a value: a
//! foreign caller wants its category, kind, and metadata, not a handle whose
//! lifetime it must manage. That also sidesteps the naming problem an object
//! would raise, since this crate's accessors return borrowed data that cannot
//! cross under the names they already have.

use std::sync::Arc;

use crate::{AnyError, ErrorFacts, MetadataDictionary, MetadataValue, make_any_error};

// --- The value layer, flattened -------------------------------------------

/// The deepest nesting this boundary will convert **on the way in**.
///
/// Far above anything a real diagnostic nests, and far below the depth that
/// threatens any normal stack — which is what lets [`value_of`] rebuild a
/// validated stream with ordinary recursion.
///
/// Encoding imposes no matching limit, because it needs none: the encoder is
/// iterative, so a Rust value nested past this depth still crosses outward
/// intact. The round trip is therefore not total — a stream that deep comes
/// back as its refusal marker — and that asymmetry is the deliberate answer. A
/// value this side already holds is not improved by refusing to show it, and
/// there is nowhere on the outbound path to report a truncation to.
pub const MAX_METADATA_DEPTH: usize = 64;

/// Everything about an error except its metadata.
///
/// Split out because metadata is what nests: an error's entries live in the
/// stream alongside it, as the children of whichever node carries this header.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ForeignErrorHeader {
    /// The error category.
    pub category: String,
    /// The numeric error kind within the category.
    pub kind: i32,
    /// The source location where the error originated.
    pub location: String,
    /// The context chain, outermost first.
    pub context: Vec<String>,
}

/// One node of a [`ForeignMetadata`] stream.
///
/// A container declares how many children follow it; those children are the next
/// complete subtrees in the stream, in order. Nothing here refers to another
/// node, so this type is not recursive and neither is lifting or dropping it.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum ForeignMetadataNode {
    /// No value.
    Null,
    /// A Boolean value.
    Boolean {
        /// The value.
        value: bool,
    },
    /// A signed 64-bit integer.
    Int64 {
        /// The value.
        value: i64,
    },
    /// An unsigned 64-bit integer.
    Uint64 {
        /// The value.
        value: u64,
    },
    /// A 64-bit floating-point value.
    Float64 {
        /// The value.
        value: f64,
    },
    /// A UTF-8 string.
    String {
        /// The value.
        value: String,
    },
    /// An ordered array; `count` element subtrees follow.
    Array {
        /// How many element subtrees follow this node.
        count: u64,
    },
    /// A dictionary in key order; one value subtree follows per key.
    Dictionary {
        /// The keys, in order, one per following subtree.
        keys: Vec<String>,
    },
    /// A nested error; one metadata value subtree follows per key.
    Error {
        /// Everything about the error except its metadata.
        header: ForeignErrorHeader,
        /// The error's metadata keys, in order, one per following subtree.
        keys: Vec<String>,
    },
}

/// The boundary form of [`MetadataValue`]: one value, as a pre-order stream.
///
/// Exactly one rooted value per stream. See the module documentation for why
/// this is flat.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ForeignMetadata {
    /// The nodes, in pre-order.
    pub nodes: Vec<ForeignMetadataNode>,
}

/// One key and its value, for building a dictionary or an error's metadata.
///
/// A building block rather than a wire shape: it holds a whole
/// [`ForeignMetadata`], which holds nodes, which hold nothing — so it nests
/// exactly twice and lifting it does not recurse.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ForeignMetadataEntry {
    /// The entry's key.
    pub key: String,
    /// The entry's value.
    pub value: ForeignMetadata,
}

/// The boundary form of [`AnyError`].
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ForeignError {
    /// Everything about the error except its metadata.
    pub header: ForeignErrorHeader,
    /// Structured metadata, as a stream whose root is a dictionary node.
    pub metadata: ForeignMetadata,
}

// --- Refusals -------------------------------------------------------------

/// The key an unusable metadata stream is reported under.
///
/// A refusal is *visible* rather than silent: a malformed stream becomes a
/// dictionary carrying this key and the reason, never an empty one. Dropping
/// data can be acceptable where hiding the drop never is, and a boundary that
/// cannot convert what it was handed is held to the same rule.
pub const REJECTED_KEY: &str = "bc_error.metadata_rejected";

const REJECTED_EMPTY: &str = "<metadata stream is empty>";
const REJECTED_MALFORMED: &str = "<metadata stream is malformed>";
const REJECTED_TOO_DEEP: &str = "<metadata nested deeper than the boundary converts>";

/// How many child subtrees follow `node`.
fn child_count(node: &ForeignMetadataNode) -> u64 {
    match node {
        ForeignMetadataNode::Array { count } => *count,
        ForeignMetadataNode::Dictionary { keys } | ForeignMetadataNode::Error { keys, .. } => {
            u64::try_from(keys.len()).unwrap_or(u64::MAX)
        }
        _ => 0,
    }
}

/// Whether `nodes` is exactly one well-formed, in-depth value.
///
/// One left-to-right pass, iterative, with a stack holding how many children
/// each open container still owes. The stack's height *is* the current depth, so
/// the depth ceiling costs nothing extra. Being iterative is not a style
/// preference: a recursive validator would be the stack overflow it exists to
/// prevent.
///
/// A declared child count is never used to reserve anything, so a node claiming
/// `u64::MAX` children costs one comparison and fails as a truncated stream.
///
/// # Errors
///
/// Returns the marker text describing why the stream is unusable: empty,
/// miscounted (truncated, or carrying trailing nodes after the root closes), or
/// nested deeper than [`MAX_METADATA_DEPTH`].
pub fn validate(nodes: &[ForeignMetadataNode]) -> Result<(), &'static str> {
    if nodes.is_empty() {
        return Err(REJECTED_EMPTY);
    }
    let mut open: Vec<u64> = Vec::new();
    let mut root_seen = false;
    for node in nodes {
        match open.last_mut() {
            Some(remaining) => *remaining -= 1,
            // The root's subtree has already closed, so this node has no parent.
            None if root_seen => return Err(REJECTED_MALFORMED),
            None => root_seen = true,
        }
        let count = child_count(node);
        if count > 0 {
            open.push(count);
            if open.len() > MAX_METADATA_DEPTH {
                return Err(REJECTED_TOO_DEEP);
            }
        }
        // Close every container this node completed.
        while open.last() == Some(&0) {
            open.pop();
        }
    }
    if open.is_empty() {
        Ok(())
    } else {
        Err(REJECTED_MALFORMED)
    }
}

// --- Encoding: Rust value to stream ---------------------------------------

fn header_of(error: &AnyError) -> ForeignErrorHeader {
    ForeignErrorHeader {
        category: error.category().to_owned(),
        kind: error.kind(),
        location: error.location().to_owned(),
        context: error.context().to_vec(),
    }
}

/// Emits `pending` in pre-order, deepest-last so the stack pops left to right.
///
/// Iterative for the same reason [`validate`] is: this side builds streams from
/// values that may themselves have arrived over the boundary.
fn emit(mut pending: Vec<&MetadataValue>, nodes: &mut Vec<ForeignMetadataNode>) {
    while let Some(value) = pending.pop() {
        match value {
            MetadataValue::Null => nodes.push(ForeignMetadataNode::Null),
            MetadataValue::Boolean(value) => {
                nodes.push(ForeignMetadataNode::Boolean { value: *value });
            }
            MetadataValue::Int64(value) => nodes.push(ForeignMetadataNode::Int64 { value: *value }),
            MetadataValue::Uint64(value) => {
                nodes.push(ForeignMetadataNode::Uint64 { value: *value });
            }
            MetadataValue::Float64(value) => {
                nodes.push(ForeignMetadataNode::Float64 { value: *value });
            }
            MetadataValue::String(value) => nodes.push(ForeignMetadataNode::String {
                value: value.clone(),
            }),
            MetadataValue::Array(values) => {
                nodes.push(ForeignMetadataNode::Array {
                    count: u64::try_from(values.len()).unwrap_or(u64::MAX),
                });
                pending.extend(values.iter().rev());
            }
            MetadataValue::Dictionary(entries) => {
                nodes.push(ForeignMetadataNode::Dictionary {
                    keys: entries.keys().cloned().collect(),
                });
                pending.extend(entries.values().rev());
            }
            MetadataValue::Error(error) => {
                nodes.push(ForeignMetadataNode::Error {
                    header: header_of(error),
                    keys: error.metadata().keys().cloned().collect(),
                });
                pending.extend(error.metadata().values().rev());
            }
        }
    }
}

/// Encodes one value as a stream.
#[must_use]
pub fn metadata_of(value: &MetadataValue) -> ForeignMetadata {
    let mut nodes = Vec::new();
    emit(vec![value], &mut nodes);
    ForeignMetadata { nodes }
}

/// Encodes a dictionary as a stream rooted at a dictionary node.
///
/// `BTreeMap` iterates in key order, so the keys are sorted by construction
/// rather than by a sort this function performs — which is what preserves the
/// ordering `stringify` and `format_message` depend on.
#[must_use]
pub fn metadata_of_dictionary(dictionary: &MetadataDictionary) -> ForeignMetadata {
    let mut nodes = vec![ForeignMetadataNode::Dictionary {
        keys: dictionary.keys().cloned().collect(),
    }];
    emit(dictionary.values().rev().collect(), &mut nodes);
    ForeignMetadata { nodes }
}

// --- Decoding: stream to Rust value ---------------------------------------

/// Rebuilds the subtree starting at `cursor`, advancing it past that subtree.
///
/// Recursive, and safely so: every caller runs [`validate`] first, which bounds
/// the depth at [`MAX_METADATA_DEPTH`]. That ordering is the whole reason the
/// stream shape exists — see the module documentation.
fn build(nodes: &[ForeignMetadataNode], cursor: &mut usize) -> MetadataValue {
    let node = &nodes[*cursor];
    *cursor += 1;
    match node {
        ForeignMetadataNode::Null => MetadataValue::Null,
        ForeignMetadataNode::Boolean { value } => MetadataValue::Boolean(*value),
        ForeignMetadataNode::Int64 { value } => MetadataValue::Int64(*value),
        ForeignMetadataNode::Uint64 { value } => MetadataValue::Uint64(*value),
        ForeignMetadataNode::Float64 { value } => MetadataValue::Float64(*value),
        ForeignMetadataNode::String { value } => MetadataValue::String(value.clone()),
        ForeignMetadataNode::Array { count } => {
            // Validation proved these elements are present, so the count is
            // bounded by the stream's own length.
            let count = usize::try_from(*count).unwrap_or(0);
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(build(nodes, cursor));
            }
            MetadataValue::Array(values)
        }
        ForeignMetadataNode::Dictionary { keys } => {
            MetadataValue::Dictionary(dictionary_from(keys, nodes, cursor))
        }
        ForeignMetadataNode::Error { header, keys } => {
            let metadata = dictionary_from(keys, nodes, cursor);
            MetadataValue::Error(Box::new(make_any_error(&CapturedFacts::from_parts(
                header, metadata,
            ))))
        }
    }
}

/// Reads one value per key, in order.
///
/// A duplicate key keeps the last occurrence, which is what inserting a sequence
/// into a map does; the foreign side is not trusted to have sent a unique-keyed
/// sequence just because this side always produces one.
fn dictionary_from(
    keys: &[String],
    nodes: &[ForeignMetadataNode],
    cursor: &mut usize,
) -> MetadataDictionary {
    let mut dictionary = MetadataDictionary::new();
    for key in keys {
        dictionary.insert(key.clone(), build(nodes, cursor));
    }
    dictionary
}

/// The value a stream encodes, or a marker naming why it was refused.
///
/// Total by construction: an unusable stream becomes a string carrying the
/// reason rather than a panic or a silent empty value.
#[must_use]
pub fn value_of(metadata: &ForeignMetadata) -> MetadataValue {
    match validate(&metadata.nodes) {
        Ok(()) => {
            let mut cursor = 0;
            build(&metadata.nodes, &mut cursor)
        }
        Err(reason) => MetadataValue::String(reason.to_owned()),
    }
}

/// The dictionary a stream encodes, when its root is a dictionary node.
///
/// A stream that is unusable, or that roots at something other than a
/// dictionary, becomes a one-entry dictionary under [`REJECTED_KEY`] — visible
/// rather than silently empty.
#[must_use]
pub fn dictionary_of(metadata: &ForeignMetadata) -> MetadataDictionary {
    match value_of(metadata) {
        MetadataValue::Dictionary(dictionary) => dictionary,
        other => std::iter::once((REJECTED_KEY.to_owned(), other)).collect(),
    }
}

impl From<&AnyError> for ForeignError {
    fn from(error: &AnyError) -> Self {
        Self {
            header: header_of(error),
            metadata: metadata_of_dictionary(error.metadata()),
        }
    }
}

impl From<ForeignError> for AnyError {
    fn from(error: ForeignError) -> Self {
        // Routed through the same constructor a Rust caller uses, so a
        // round-tripped error is built exactly as a native one is.
        let metadata = dictionary_of(&error.metadata);
        make_any_error(&CapturedFacts::from_parts(&error.header, metadata))
    }
}

// --- The foreign-adapter layer -------------------------------------------

/// [`ErrorFacts`] implemented on the foreign side.
///
/// The Rust trait returns borrowed data — `&str`, `&MetadataDictionary`,
/// `&[String]` — which a callback interface cannot: the foreign side owns its
/// values and has nothing for Rust to borrow from. So this trait returns owned
/// values, and [`CapturedFacts`] stores one call's worth so the borrowing
/// contract can still be satisfied on this side.
#[uniffi::export(with_foreign)]
pub trait ForeignErrorFacts: Send + Sync {
    /// Returns the error category.
    fn category(&self) -> String;
    /// Returns the numeric error kind within the category.
    fn kind(&self) -> i32;
    /// Returns the source location where the error originated.
    fn location(&self) -> String;
    /// Returns structured metadata as a stream rooted at a dictionary node.
    fn metadata(&self) -> ForeignMetadata;
    /// Returns the context chain, outermost first.
    fn context(&self) -> Vec<String>;
}

/// One reading of a [`ForeignErrorFacts`], stored so it can be borrowed.
///
/// Each accessor on the foreign trait is a boundary crossing that may allocate,
/// so this reads every field **once** and then serves [`ErrorFacts`] from what
/// it read. That also makes the snapshot internally consistent: a foreign
/// object that changed between two calls cannot yield an error whose category
/// and metadata disagree.
#[derive(Clone, Debug)]
pub struct CapturedFacts {
    category: String,
    kind: i32,
    location: String,
    metadata: MetadataDictionary,
    context: Vec<String>,
}

impl CapturedFacts {
    /// Reads every field of `source` once.
    #[must_use]
    pub fn capture(source: &dyn ForeignErrorFacts) -> Self {
        Self {
            category: source.category(),
            kind: source.kind(),
            location: source.location(),
            metadata: dictionary_of(&source.metadata()),
            context: source.context(),
        }
    }

    /// Assembles facts from a header and an already-decoded dictionary.
    #[must_use]
    pub fn from_parts(header: &ForeignErrorHeader, metadata: MetadataDictionary) -> Self {
        Self {
            category: header.category.clone(),
            kind: header.kind,
            location: header.location.clone(),
            metadata,
            context: header.context.clone(),
        }
    }
}

impl ErrorFacts for CapturedFacts {
    fn category(&self) -> &str {
        &self.category
    }

    fn kind(&self) -> i32 {
        self.kind
    }

    fn location(&self) -> &str {
        &self.location
    }

    fn metadata(&self) -> &MetadataDictionary {
        &self.metadata
    }

    fn context(&self) -> &[String] {
        &self.context
    }
}

// --- Exported constructors ------------------------------------------------
//
// A stream is tedious to assemble by hand, so nothing foreign has to: each of
// these returns a complete value, and composing them concatenates streams in
// the pre-order the format wants.

/// A stream holding [`MetadataValue::Null`].
#[uniffi::export]
#[must_use]
pub fn metadata_null() -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Null],
    }
}

/// A stream holding one Boolean.
#[uniffi::export]
#[must_use]
pub fn metadata_boolean(value: bool) -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Boolean { value }],
    }
}

/// A stream holding one signed 64-bit integer.
#[uniffi::export]
#[must_use]
pub fn metadata_int64(value: i64) -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Int64 { value }],
    }
}

/// A stream holding one unsigned 64-bit integer.
#[uniffi::export]
#[must_use]
pub fn metadata_uint64(value: u64) -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Uint64 { value }],
    }
}

/// A stream holding one 64-bit floating-point value.
#[uniffi::export]
#[must_use]
pub fn metadata_float64(value: f64) -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Float64 { value }],
    }
}

/// A stream holding one string.
#[uniffi::export]
#[must_use]
pub fn metadata_string(value: String) -> ForeignMetadata {
    ForeignMetadata {
        nodes: vec![ForeignMetadataNode::String { value }],
    }
}

/// A stream holding an array of `elements`, in order.
#[uniffi::export]
#[must_use]
pub fn metadata_array(elements: Vec<ForeignMetadata>) -> ForeignMetadata {
    let mut nodes = vec![ForeignMetadataNode::Array {
        count: u64::try_from(elements.len()).unwrap_or(u64::MAX),
    }];
    for element in elements {
        nodes.extend(element.nodes);
    }
    ForeignMetadata { nodes }
}

/// A stream holding a dictionary of `entries`.
///
/// Sorted by key here rather than trusted from the caller, so a stream this
/// crate builds is always in the key order the Rust side guarantees. A duplicate
/// key survives into the stream and is resolved on decode, last one winning.
#[uniffi::export]
#[must_use]
pub fn metadata_dictionary(entries: Vec<ForeignMetadataEntry>) -> ForeignMetadata {
    let (keys, children) = flatten_entries(entries);
    let mut nodes = vec![ForeignMetadataNode::Dictionary { keys }];
    nodes.extend(children);
    ForeignMetadata { nodes }
}

/// A stream holding a nested error with `entries` as its metadata.
#[uniffi::export]
#[must_use]
pub fn metadata_error(
    header: ForeignErrorHeader,
    entries: Vec<ForeignMetadataEntry>,
) -> ForeignMetadata {
    let (keys, children) = flatten_entries(entries);
    let mut nodes = vec![ForeignMetadataNode::Error { header, keys }];
    nodes.extend(children);
    ForeignMetadata { nodes }
}

/// Sorts entries by key and concatenates their streams in that order.
fn flatten_entries(
    mut entries: Vec<ForeignMetadataEntry>,
) -> (Vec<String>, Vec<ForeignMetadataNode>) {
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    let mut keys = Vec::with_capacity(entries.len());
    let mut nodes = Vec::new();
    for entry in entries {
        keys.push(entry.key);
        nodes.extend(entry.value.nodes);
    }
    (keys, nodes)
}

// --- Exported functions ---------------------------------------------------

/// Builds an error from facts a foreign caller implements.
///
/// The foreign counterpart of [`make_any_error`], which takes any Rust
/// [`ErrorFacts`]. Both routes end in the same constructor.
#[uniffi::export]
#[must_use]
// The owned `Arc` is UniFFI's calling convention for a foreign-implemented
// trait, not a choice this signature is free to make: taking `&Arc` here would
// not compile as an exported function.
#[allow(clippy::needless_pass_by_value)]
pub fn error_from_foreign_facts(facts: Arc<dyn ForeignErrorFacts>) -> ForeignError {
    ForeignError::from(&make_any_error(&CapturedFacts::capture(facts.as_ref())))
}

/// Renders an error the way this crate's human diagnostic does.
#[uniffi::export]
#[must_use]
// UniFFI lifts a record argument into an owned value; a reference here is not
// this signature's to choose. Same for the three functions below.
#[allow(clippy::needless_pass_by_value)]
pub fn format_error_message(error: ForeignError) -> String {
    crate::format_error_message(&AnyError::from(error))
}

/// Renders a value the way this crate's diagnostic does.
///
/// A stream this boundary cannot convert renders as the marker naming why, which
/// is also what it would be stored as — see [`validate`].
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn stringify_metadata_value(metadata: ForeignMetadata) -> String {
    value_of(&metadata).stringify()
}

/// Reports which [`crate::MetadataType`] a stream converts to.
///
/// Validated first, then read from the root node's tag. Validating is what makes
/// this agree with [`value_of`] on *every* stream rather than only on the ones
/// that convert: a refused stream reads as `String`, because the marker it
/// converts to is a string.
///
/// Reading the tag alone was cheaper and wrong — it answered `Array` for a
/// stream [`stringify_metadata_value`] refused, and `Null` for an empty one that
/// decodes to a marker, so two exported functions disagreed about one value. The
/// cost of the fix is one linear pass; the "rebuilding the tree would be
/// allocation for nothing" argument that justified the tag read applies to
/// rebuilding the value, not to [`validate`], which allocates only the depth
/// stack.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn metadata_value_type(metadata: ForeignMetadata) -> crate::MetadataType {
    if validate(&metadata.nodes).is_err() {
        return crate::MetadataType::String;
    }
    match metadata.nodes.first() {
        None | Some(ForeignMetadataNode::Null) => crate::MetadataType::Null,
        Some(ForeignMetadataNode::Boolean { .. }) => crate::MetadataType::Boolean,
        Some(ForeignMetadataNode::Int64 { .. }) => crate::MetadataType::Int64,
        Some(ForeignMetadataNode::Uint64 { .. }) => crate::MetadataType::Uint64,
        Some(ForeignMetadataNode::Float64 { .. }) => crate::MetadataType::Float64,
        Some(ForeignMetadataNode::String { .. }) => crate::MetadataType::String,
        Some(ForeignMetadataNode::Array { .. }) => crate::MetadataType::Array,
        Some(ForeignMetadataNode::Dictionary { .. }) => crate::MetadataType::Dictionary,
        Some(ForeignMetadataNode::Error { .. }) => crate::MetadataType::Error,
    }
}

/// Why a stream cannot be converted, or `None` when it can.
///
/// Exported so a foreign caller can check a stream it assembled before handing
/// it to anything that would substitute a marker for it.
#[uniffi::export]
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn metadata_rejection(metadata: ForeignMetadata) -> Option<String> {
    validate(&metadata.nodes).err().map(str::to_owned)
}
