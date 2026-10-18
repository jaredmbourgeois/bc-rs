//! The files one run read, and positions within them.
//!
//! Every diagnostic names `file:line:column`, so every value the front ends
//! produce carries a [`Span`] back into the text it came from. Spans are byte
//! offsets — what both parsers report — and become a line and a column only when
//! a diagnostic is rendered.

use std::path::{Path, PathBuf};

/// One file in a [`Sources`] set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FileId(usize);

/// A byte range within one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) file: FileId,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    /// The empty span at the very start of `file`, for a fact about the file as
    /// a whole (its root table has no key to point at).
    pub(crate) fn start_of(file: FileId) -> Self {
        Self {
            file,
            start: 0,
            end: 0,
        }
    }
}

/// A value together with where it was written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spanned<T> {
    pub(crate) value: T,
    pub(crate) span: Span,
}

/// One file's display path and text.
#[derive(Debug)]
pub(crate) struct SourceFile {
    /// The path as diagnostics print it — relative to where the run started
    /// when the file was reached through a relative path.
    pub(crate) display_path: PathBuf,
    pub(crate) text: String,
}

/// A one-based line and column, the column counted in characters so it lines
/// up with what an editor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineColumn {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

/// Every file a run read, addressed by [`FileId`].
#[derive(Debug, Default)]
pub(crate) struct Sources {
    files: Vec<SourceFile>,
}

impl Sources {
    pub(crate) fn add(&mut self, display_path: PathBuf, text: String) -> FileId {
        self.files.push(SourceFile { display_path, text });
        FileId(self.files.len() - 1)
    }

    pub(crate) fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.0]
    }

    pub(crate) fn display_path(&self, id: FileId) -> &Path {
        &self.file(id).display_path
    }

    /// Where `span` starts, as a message names it: `file:line:column`.
    pub(crate) fn describe(&self, span: Span) -> String {
        let position = self.line_column(span);
        format!(
            "{}:{}:{}",
            self.display_path(span.file).display(),
            position.line,
            position.column
        )
    }

    /// Where `span` starts, as a line and a column. Counted by walking the
    /// characters before it, so an offset is never used to slice the text.
    pub(crate) fn line_column(&self, span: Span) -> LineColumn {
        let mut position = LineColumn { line: 1, column: 1 };
        for (index, character) in self.file(span.file).text.char_indices() {
            if index >= span.start {
                break;
            }
            if character == '\n' {
                position.line += 1;
                position.column = 1;
            } else {
                position.column += 1;
            }
        }
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_count_lines_and_characters() {
        let mut sources = Sources::default();
        let file = sources.add(PathBuf::from("a.toml"), "ab\ncé = 1\n".to_string());
        let at = |start| {
            sources.line_column(Span {
                file,
                start,
                end: start,
            })
        };
        assert_eq!(at(0), LineColumn { line: 1, column: 1 });
        assert_eq!(at(3), LineColumn { line: 2, column: 1 });
        // `é` is two bytes and one column, so `=` (byte 7) is column 4.
        assert_eq!(at(7), LineColumn { line: 2, column: 4 });
        assert_eq!(
            sources.describe(Span {
                file,
                start: 7,
                end: 8
            }),
            "a.toml:2:4"
        );
    }
}
