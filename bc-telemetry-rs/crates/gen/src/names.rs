//! The name grammar.
//!
//! Two shapes, by whether a name becomes a language identifier. Modules, items,
//! attributes, fields and events are `snake_case`, because adapters emit them as
//! identifiers; queries and dashboards are `kebab-case`, because neither is
//! one directly. Neither shape admits a `.`, which is what makes a qualified
//! name split one way and keeps the reserved `bc.telemetry.*` attributes every
//! generated record carries out of reach of every declaration. A snake name
//! never holds `__` either, so `__` is free to separate the parts of a view
//! name.
//!
//! Two names are derived from declared ones and held to the limit of the
//! engine that will carry them: a view's name and a dashboard's uid.

/// Which grammar a name is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameShape {
    Snake,
    Kebab,
}

impl NameShape {
    fn separator(self) -> char {
        match self {
            Self::Snake => '_',
            Self::Kebab => '-',
        }
    }

    /// The grammar as an error message states it.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Snake => {
                "snake_case: lowercase letters and digits in `_`-separated words, starting with a letter"
            }
            Self::Kebab => {
                "kebab-case: lowercase letters and digits in `-`-separated words, starting with a letter"
            }
        }
    }

    /// Whether `name` fits: one or more non-empty words of `[a-z0-9]`, joined by
    /// single separators, the first character a letter.
    pub(crate) fn admits(self, name: &str) -> bool {
        let starts_with_letter = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase());
        starts_with_letter
            && name.split(self.separator()).all(|word| {
                !word.is_empty()
                    && word.chars().all(|character| {
                        character.is_ascii_lowercase() || character.is_ascii_digit()
                    })
            })
    }
}

/// PostgreSQL keeps 63 bytes of an identifier and silently drops the rest, so
/// two longer view names could become one view, and two longer attribute names
/// one column of it.
pub(crate) const IDENTIFIER_BYTES: usize = 63;

/// Grafana refuses a dashboard uid past 40 characters.
pub(crate) const DASHBOARD_UID_CHARACTERS: usize = 40;

/// The name of an item's typed view: `<kind>__<module>__<item>`, where
/// `kind` is the item kind's keyword.
pub(crate) fn view_name(kind: &str, module: &str, item: &str) -> String {
    format!("{kind}__{module}__{item}")
}

/// A dashboard's Grafana uid: `<module>-<dashboard>`, so two modules'
/// dashboards of one name never meet in one Grafana.
pub(crate) fn dashboard_uid(module: &str, dashboard: &str) -> String {
    format!("{module}-{dashboard}")
}

#[cfg(test)]
mod tests {
    use super::{NameShape, dashboard_uid, view_name};

    #[test]
    fn snake_names_admit_words_and_digits_and_refuse_everything_else() {
        for name in ["checkout", "request_duration", "http_2xx", "p95"] {
            assert!(NameShape::Snake.admits(name), "{name}");
        }
        for name in [
            "",
            "Checkout",
            "2xx",
            "_leading",
            "trailing_",
            "double__under",
            "kebab-name",
            "dotted.name",
            "spa ce",
        ] {
            assert!(!NameShape::Snake.admits(name), "{name}");
        }
    }

    #[test]
    fn derived_names_join_their_parts_with_separators_no_name_can_hold() {
        assert_eq!(
            view_name("metric", "store_server", "request_duration"),
            "metric__store_server__request_duration"
        );
        assert_eq!(
            dashboard_uid("store_server", "checkout-health"),
            "store_server-checkout-health"
        );
    }

    #[test]
    fn kebab_names_are_the_same_grammar_with_a_hyphen() {
        assert!(NameShape::Kebab.admits("checkout-p95"));
        assert!(NameShape::Kebab.admits("store-server"));
        assert!(!NameShape::Kebab.admits("snake_name"));
        assert!(!NameShape::Kebab.admits("-leading"));
        assert!(!NameShape::Kebab.admits("bc.telemetry"));
    }
}
