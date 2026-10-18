//! The language adapters: each takes the model and emits one language's source
//! into every module's own directory, at the root's `output` taken relative to
//! it, since that code is compiled with the module's own.
//!
//! What every adapter shares is here: the languages `[generate] languages`
//! names, and the names a language cannot take, by the role a name plays in
//! the code it generates — refused at lowering, where each name still has its
//! place in its file, rather than found by the consumer's compiler.

pub(crate) mod cpp;
pub(crate) mod rust;

use std::collections::BTreeSet;

use crate::model::{ItemKind, Keyword};

/// A language an adapter emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Language {
    Cpp,
    Rust,
}

impl Keyword for Language {
    const ALL: &'static [Self] = &[Self::Cpp, Self::Rust];

    fn keyword(self) -> &'static str {
        match self {
            Self::Cpp => "cpp",
            Self::Rust => "rust",
        }
    }
}

impl Language {
    /// The language as a message names it.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Cpp => "C++",
            Self::Rust => "Rust",
        }
    }
}

/// What a declared name becomes in generated code, which decides what it may
/// not be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameRole {
    /// A span, metric, log or error: a function, and a type.
    Item(ItemKind),
    /// An attribute or a field: a parameter of its item's constructor.
    Attribute(ItemKind),
    /// A span's event: a method on each of its span's handles.
    Event,
}

/// The first of `languages` in which `name` cannot play `role`, if any.
pub(crate) fn reserved_in(
    languages: &BTreeSet<Language>,
    role: NameRole,
    name: &str,
) -> Option<Language> {
    languages.iter().copied().find(|language| match language {
        Language::Cpp => cpp::is_reserved(role, name),
        Language::Rust => rust::is_reserved(role, name),
    })
}
