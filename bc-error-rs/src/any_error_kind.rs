use std::borrow::Cow;

/// A named error kind: a category paired with its numeric value.
///
/// The two together are what identifies an error. Held apart — as a loose
/// `(&str, i32)` pair passed positionally — nothing stops one category's name
/// reaching a call beside another category's value, and nothing stops a message
/// landing in the slot after them. Held together in one value, both mistakes
/// stop being expressible, which is the whole reason this type exists.
///
/// It deliberately depends on nothing else in this crate, so a domain can
/// declare its registry of named kinds without reaching for [`AnyError`],
/// metadata, or a `Result`:
///
/// ```
/// use bc_error::AnyErrorKind;
///
/// const MISSING_FIELD: AnyErrorKind = AnyErrorKind::new("json", 1);
/// const TYPE_MISMATCH: AnyErrorKind = AnyErrorKind::new("json", 2);
///
/// assert_eq!(MISSING_FIELD.category(), "json");
/// assert_ne!(MISSING_FIELD, TYPE_MISMATCH);
/// ```
///
/// [`AnyError`]: crate::AnyError
// Deliberately not `Serialize`/`Deserialize`. A descriptor is how a call site
// *names* an error, not part of one: what crosses the wire is the assembled
// `AnyError`, whose payload already carries the category and the value as its
// own fields. A derive here would be public surface with no consumer and no
// test — and this crate has shipped exactly that before, five serde tests that
// had never executed. Add it when something needs to serialize a kind on its
// own, with the round-trip test in the same change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnyErrorKind {
    category: Cow<'static, str>,
    value: i32,
}

impl AnyErrorKind {
    /// Declares a kind whose category is a literal, usable in a `const`.
    ///
    /// This is the registry constructor — the form the source library spells
    /// `constexpr AnyErrorKind`. It borrows, so a declared kind allocates
    /// nothing.
    #[must_use]
    pub const fn new(category: &'static str, value: i32) -> Self {
        Self {
            category: Cow::Borrowed(category),
            value,
        }
    }

    /// Builds a kind whose category is only known at run time.
    ///
    /// Needed where a category arrives as data rather than as a literal — the
    /// path [`make_any_error`] takes when it erases a foreign error whose
    /// category it cannot name at compile time.
    ///
    /// [`make_any_error`]: crate::make_any_error
    #[must_use]
    pub fn owned(category: impl Into<String>, value: i32) -> Self {
        Self {
            category: Cow::Owned(category.into()),
            value,
        }
    }

    /// Returns this kind's category.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    /// Returns this kind's numeric value within its category.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    /// Consumes this kind and returns its category, reusing the allocation when
    /// there is one.
    pub(crate) fn into_category(self) -> Cow<'static, str> {
        self.category
    }
}

/// A condition that is a **refusal** every time it happens: the operation
/// refused its inputs, and the same inputs are refused the same way until
/// someone changes the data or the configuration — no printer is registered,
/// nothing prices the filament. Retrying cannot cure it.
///
/// Permanence is a property of the condition, so it is declared where the
/// condition is: a registry declares a refusal with this type instead of
/// [`AnyErrorKind`], and every [`AnyError`] built from it carries the `refused`
/// fact ([`is_refusal`]) by construction. No raise site restates it and none can
/// forget it.
///
/// A separate type rather than a flag on [`AnyErrorKind`], with no conversion
/// to one: nothing can turn a refusal into a plain kind and quietly drop the
/// fact on the way to a factory.
///
/// ```
/// use bc_error::{AnyError, RefusalKind, is_refusal};
///
/// const NO_PRINTER: RefusalKind = RefusalKind::new("labels", 4);
///
/// assert!(is_refusal(&AnyError::from_kind(NO_PRINTER)));
/// ```
///
/// [`AnyError`]: crate::AnyError
/// [`is_refusal`]: crate::is_refusal
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RefusalKind {
    category: &'static str,
    value: i32,
}

impl RefusalKind {
    /// Declares a refusal condition, usable in a `const`.
    #[must_use]
    pub const fn new(category: &'static str, value: i32) -> Self {
        Self { category, value }
    }

    /// Returns this condition's category.
    #[must_use]
    pub const fn category(&self) -> &'static str {
        self.category
    }

    /// Returns this condition's numeric value within its category.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }
}

/// What an error is raised as: a plain [`AnyErrorKind`], or a [`RefusalKind`],
/// whose every error carries the `refused` fact. The factories of
/// [`AnyError`](crate::AnyError) take either, as the source's overloads do.
///
/// Sealed: these two are the whole set, so a third that forgot the refusal
/// cannot be written outside this crate.
pub trait ErrorCondition: sealed::Sealed {}

impl ErrorCondition for AnyErrorKind {}
impl ErrorCondition for RefusalKind {}

pub(crate) mod sealed {
    use std::borrow::Cow;

    /// What a factory reads from a condition. Unnameable outside this crate,
    /// which is what seals [`ErrorCondition`](super::ErrorCondition).
    pub trait Sealed {
        fn category(&self) -> &str;
        fn value(&self) -> i32;
        /// Whether every error raised as this condition is a refusal.
        fn refuses(&self) -> bool;
        fn into_category(self) -> Cow<'static, str>;
    }

    impl Sealed for super::AnyErrorKind {
        fn category(&self) -> &str {
            self.category()
        }
        fn value(&self) -> i32 {
            self.value()
        }
        fn refuses(&self) -> bool {
            false
        }
        fn into_category(self) -> Cow<'static, str> {
            self.into_category()
        }
    }

    impl Sealed for super::RefusalKind {
        fn category(&self) -> &str {
            self.category
        }
        fn value(&self) -> i32 {
            self.value
        }
        fn refuses(&self) -> bool {
            true
        }
        fn into_category(self) -> Cow<'static, str> {
            Cow::Borrowed(self.category)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AnyErrorKind;

    #[test]
    fn a_declared_kind_borrows_its_category() {
        const DECLARED: AnyErrorKind = AnyErrorKind::new("net", 404);

        assert_eq!(DECLARED.category(), "net");
        assert_eq!(DECLARED.value(), 404);
    }

    #[test]
    fn an_owned_category_compares_equal_to_a_borrowed_one() {
        assert_eq!(
            AnyErrorKind::owned(String::from("net"), 404),
            AnyErrorKind::new("net", 404)
        );
    }

    #[test]
    fn kinds_differing_only_in_category_are_not_equal() {
        // The pairing is the point: the same number under a different category
        // is a different error, which a bare `i32` comparison would miss.
        assert_ne!(
            AnyErrorKind::new("http", 404),
            AnyErrorKind::new("net", 404)
        );
    }
}
