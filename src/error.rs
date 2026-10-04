//! Typed, exhaustive error taxonomy for the XLSX codec.
//!
//! Estate error discipline (see `wire-kit`): the enum is **exhaustive** (no
//! `#[non_exhaustive]`) and decoding is total — malformed packages yield a
//! variant, never a panic. Sources are preserved via [`std::error::Error::source`]
//! for the wrapped `zip` and `quick-xml` errors.

use std::fmt;

/// Everything that can go wrong while reading or writing an XLSX package.
#[derive(Debug)]
pub enum XlsxError {
    /// The container is not a readable ZIP archive, or a required part is
    /// missing from it.
    Zip(zip::result::ZipError),
    /// A required package part is absent (located through the relationship
    /// graph, so this fires for broken packages, not renamed ones).
    MissingPart {
        /// The part path that was required.
        part: String,
    },
    /// An XML part could not be parsed.
    Xml {
        /// Which part failed (for diagnostics).
        part: String,
        /// The underlying parser error.
        source: quick_xml::errors::Error,
    },
    /// The workbook's relationship graph is broken: a `<sheet>` refers to a
    /// relationship id with no matching target.
    MissingRelationship {
        /// The dangling relationship id.
        id: String,
    },
    /// A cell part is structurally malformed beyond tolerance: an invalid
    /// `r` reference, an unparseable number in a numeric cell, a bad
    /// boolean literal, or a shared-string index that is not a number.
    CorruptCell {
        /// The cell reference as written (`"B3"`), or the row context when
        /// absent.
        reference: String,
        /// What was wrong with it.
        reason: &'static str,
    },
    /// A cell points past the end of the shared-string table.
    SharedStringRange {
        /// The offending index.
        index: u32,
        /// The table length.
        len: usize,
    },
    /// `write_xlsx` was handed a non-finite number (`NaN`, ±∞). XLSX has no
    /// representation for them; the model refuses to write a lie.
    NonFiniteNumber(f64),
}

impl fmt::Display for XlsxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XlsxError::Zip(e) => write!(f, "zip container error: {e}"),
            XlsxError::MissingPart { part } => write!(f, "missing package part: {part}"),
            XlsxError::Xml { part, source } => write!(f, "xml parse error in {part}: {source}"),
            XlsxError::MissingRelationship { id } => {
                write!(f, "workbook sheet references unknown relationship {id:?}")
            }
            XlsxError::CorruptCell { reference, reason } => {
                write!(f, "corrupt cell {reference}: {reason}")
            }
            XlsxError::SharedStringRange { index, len } => {
                write!(
                    f,
                    "shared string index {index} out of range (table holds {len})"
                )
            }
            XlsxError::NonFiniteNumber(n) => write!(f, "cannot write non-finite number {n:?}"),
        }
    }
}

impl std::error::Error for XlsxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            XlsxError::Zip(e) => Some(e),
            XlsxError::Xml { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<zip::result::ZipError> for XlsxError {
    fn from(e: zip::result::ZipError) -> Self {
        XlsxError::Zip(e)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::XlsxError;
    use std::string::{String, ToString};

    #[test]
    fn display_is_informative_and_total() {
        let cases = [
            XlsxError::MissingPart {
                part: String::from("xl/workbook.xml"),
            },
            XlsxError::MissingRelationship {
                id: String::from("rId9"),
            },
            XlsxError::CorruptCell {
                reference: String::from("B3"),
                reason: "bad number",
            },
            XlsxError::SharedStringRange { index: 7, len: 2 },
            XlsxError::NonFiniteNumber(f64::NAN),
        ];
        for e in cases {
            let s = e.to_string();
            assert!(!s.is_empty(), "{e:?} rendered empty");
        }
        // Wrapped sources keep their chain.
        let zip_err = zip::result::ZipError::InvalidArchive(std::borrow::Cow::Borrowed("nope"));
        let e = XlsxError::Zip(zip_err);
        assert!(e.to_string().contains("zip"));
        assert!(std::error::Error::source(&e).is_some());
    }
}
