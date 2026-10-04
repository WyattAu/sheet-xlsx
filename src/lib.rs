//! XLSX codec — workbook, worksheets, shared strings, styles, calc chain —
//! parse and serialize.
//!
//! `sheet-xlsx` reads and writes `SpreadsheetML` packages (`.xlsx`) on top of
//! the estate's [`sheet-core`] data model: [`XlsxWorkbook`] is the package
//! view (named sheets, shared-string table, style count), and every cell
//! coordinate is `(row, col)` in the shared 0-based space, with `A1`
//! conversion borrowed from `sheet_core::refs`.
//!
//! # Package layout
//!
//! A workbook is a ZIP of XML parts. On write, `sheet-xlsx` emits the
//! canonical minimal set — `[Content_Types].xml`, `_rels/.rels`,
//! `xl/workbook.xml`, its relationships, one `xl/worksheets/sheetN.xml` per
//! sheet, `xl/sharedStrings.xml`, `xl/styles.xml`, and `xl/calcChain.xml`
//! when any cell carries a formula. On read, parts are located through the
//! relationship graph (not by hardcoded paths), so renamed or relocated
//! packages still parse.
//!
//! # Faithfulness over cleverness
//!
//! The codec is deliberately **lossless for the model it defines**:
//! round-tripping a workbook through `write_xlsx` → `read_xlsx` reproduces
//! `XlsxCell` values exactly — including the choice between shared and
//! inline strings, error literals, cached formula results, and style
//! indices — which is the property the test suite checks on random
//! workbooks. Full OOXML (charts, pivot tables, themes, merged cells,
//! number formats) is out of scope: only the parts listed above are
//! consumed or emitted, and the reader ignores everything else.
//!
//! # Known v0.1 limitations (documented, typed, non-panicking)
//!
//! - **Style definitions are not modeled.** Styles are a count of `cellXfs`
//!   entries; indices survive round-trips, but fonts/fills/borders do not.
//! - **Shared formulas**: the master cell keeps its formula text; slave
//!   cells keep their cached values but their translated formulas are not
//!   reconstructed.
//! - **Cells with neither value nor formula** (e.g. style-only cells) are
//!   dropped — absence is blank, matching `sheet-core`.
//!
//! # Layer
//!
//! L1 substrate: depends on [`sheet-core`] (L1, same layer) plus `zip` and
//! `quick-xml`; `#![forbid(unsafe_code)]`.
//!
//! # Example
//!
//! ```
//! use sheet_xlsx::{read_xlsx, write_xlsx, XlsxCell, SheetData, XlsxValue, XlsxWorkbook};
//! use std::collections::BTreeMap;
//!
//! let mut cells = BTreeMap::new();
//! cells.insert((0, 0), XlsxCell { value: XlsxValue::Number(41.0), style_index: None, formula: None });
//! cells.insert((1, 0), XlsxCell {
//!     value: XlsxValue::FormulaString(String::new()),
//!     style_index: None,
//!     formula: Some(String::from("=A1+1")),
//! });
//!
//! let mut wb = XlsxWorkbook::default();
//! wb.sheets.push(SheetData { name: String::from("Sheet1"), cells });
//!
//! let bytes = write_xlsx(&wb).unwrap();
//! let round = read_xlsx(&bytes).unwrap();
//! assert_eq!(round.sheets.len(), 1);
//! assert_eq!(round.sheets[0].cells.get(&(1, 0)).unwrap().formula.as_deref(), Some("=A1+1"));
//! ```

#![forbid(unsafe_code)]

pub mod calc_chain;
pub mod error;
pub mod reader;
pub mod shared_strings;
pub mod styles;
pub mod writer;

pub use calc_chain::{CalcChain, CalcEntry};
pub use error::XlsxError;
pub use reader::read_xlsx;
pub use shared_strings::SharedStrings;
pub use styles::Styles;
pub use writer::write_xlsx;

use std::collections::BTreeMap;

/// A parsed XLSX package: named sheets in workbook order, the shared-string
/// table, and the style-table size.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XlsxWorkbook {
    /// Worksheets in workbook (`<sheet>` element) order.
    pub sheets: Vec<SheetData>,
    /// The shared-string table; [`XlsxValue::SharedString`] indexes into it.
    pub shared_strings: SharedStrings,
    /// Style-table metadata (see [`Styles`]).
    pub styles: Styles,
}

impl XlsxWorkbook {
    /// A workbook with one empty sheet named `Sheet1`.
    #[must_use]
    pub fn single_sheet() -> Self {
        XlsxWorkbook {
            sheets: vec![SheetData::new("Sheet1")],
            shared_strings: SharedStrings::default(),
            styles: Styles::default(),
        }
    }

    /// Resolves a shared-string index to its text.
    #[must_use]
    pub fn shared_text(&self, index: u32) -> Option<&str> {
        self.shared_strings.get(index)
    }
}

/// One worksheet: its name and sparse cell storage. Coordinates are 0-based
/// `(row, col)`; iteration is row-major (deterministic) via the `BTreeMap`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SheetData {
    /// The sheet name as declared in `xl/workbook.xml`.
    pub name: String,
    /// Present cells keyed by `(row, col)`.
    pub cells: BTreeMap<(u32, u32), XlsxCell>,
}

impl SheetData {
    /// An empty sheet with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        SheetData {
            name: name.into(),
            cells: BTreeMap::new(),
        }
    }
}

/// One cell: value, optional style index, optional formula text.
///
/// `formula` carries the formula **without** its cached result — the cached
/// result lives in `value` (e.g. `XlsxValue::Number` for a numeric result,
/// `XlsxValue::FormulaString` for a string result), exactly as `SpreadsheetML`
/// stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct XlsxCell {
    /// The cell's (possibly cached) value.
    pub value: XlsxValue,
    /// Index into the workbook's style table (`s` attribute), if styled.
    pub style_index: Option<u32>,
    /// Formula text without the leading `=`'s result — e.g. `"A1+1"` for
    /// `=A1+1`. Stored verbatim after the `=`.
    pub formula: Option<String>,
}

impl XlsxCell {
    /// A value-only cell.
    #[must_use]
    pub const fn new(value: XlsxValue) -> Self {
        XlsxCell {
            value,
            style_index: None,
            formula: None,
        }
    }

    /// A formula cell with a cached value.
    #[must_use]
    pub fn formula(formula: impl Into<String>, cached: XlsxValue) -> Self {
        XlsxCell {
            value: cached,
            style_index: None,
            formula: Some(formula.into()),
        }
    }
}

/// The value of an [`XlsxCell`], mirroring `SpreadsheetML`'s cell types.
#[derive(Debug, Clone, PartialEq)]
pub enum XlsxValue {
    /// A numeric cell (`t="n"`, the default type).
    Number(f64),
    /// A shared-string cell (`t="s"`): an index into the workbook's
    /// shared-string table.
    SharedString(u32),
    /// An inline string (`t="inlineStr"`): text stored in the sheet part
    /// itself. `write_xlsx` preserves the shared/inline distinction so
    /// round-trips are exact.
    InlineString(String),
    /// A boolean cell (`t="b"`).
    Boolean(bool),
    /// An error cell (`t="e"`): the literal as stored (`"#REF!"`, …).
    Error(String),
    /// A formula's cached string result (`t="str"`), or a formula cell
    /// with no cached result (empty string — see the reader docs).
    FormulaString(String),
}

impl XlsxValue {
    /// The text of string-ish values as an owned `String`
    /// ([`SharedString`](XlsxValue::SharedString) resolves through the
    /// shared table; inline and formula strings copy from the cell).
    /// `None` for numbers, booleans, and errors.
    #[must_use]
    pub fn text(&self, shared: &SharedStrings) -> Option<String> {
        match self {
            XlsxValue::SharedString(i) => shared.get(*i).map(String::from),
            XlsxValue::InlineString(s) | XlsxValue::FormulaString(s) => Some(s.clone()),
            XlsxValue::Number(_) | XlsxValue::Boolean(_) | XlsxValue::Error(_) => None,
        }
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

    use super::{SharedStrings, SheetData, Styles, XlsxValue, XlsxWorkbook};

    #[test]
    fn single_sheet_and_defaults() {
        let wb = XlsxWorkbook::single_sheet();
        assert_eq!(wb.sheets.len(), 1);
        assert_eq!(wb.sheets[0].name, "Sheet1");
        assert!(wb.sheets[0].cells.is_empty());
        assert!(wb.shared_strings.is_empty());
        assert_eq!(wb.styles.cell_xfs, 0);

        let empty = XlsxWorkbook::default();
        assert_eq!(empty.sheets.len(), 0);
        assert_eq!(SheetData::new("N").name, "N");
    }

    #[test]
    fn text_resolution_through_the_shared_table() {
        let mut shared = SharedStrings::default();
        let idx = shared.insert("hello");
        assert_eq!(idx, 0);
        let wb_idx = XlsxValue::SharedString(idx);
        assert_eq!(wb_idx.text(&shared), Some(String::from("hello")));
        assert_eq!(XlsxValue::SharedString(9).text(&shared), None);
        assert_eq!(
            XlsxValue::InlineString("s".to_string()).text(&shared),
            Some(String::from("s"))
        );
        assert_eq!(
            XlsxValue::FormulaString("f".to_string()).text(&shared),
            Some(String::from("f"))
        );
        assert_eq!(XlsxValue::Number(1.0).text(&shared), None);
        assert_eq!(XlsxValue::Boolean(true).text(&shared), None);
        assert_eq!(XlsxValue::Error("#REF!".to_string()).text(&shared), None);
    }

    #[test]
    fn shared_string_table_semantics() {
        let mut t = SharedStrings::default();
        assert!(t.is_empty());
        let a = t.insert("dup");
        let b = t.insert("dup");
        let c = t.insert("other");
        assert_eq!(a, b, "dedup");
        assert_eq!(c, 1);
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(0), Some("dup"));
        assert_eq!(t.get(1), Some("other"));
        assert_eq!(t.get(2), None);
        let round = XlsxWorkbook::default();
        let _ = round; // Default derives cleanly with all fields
        let _ = Styles::default();
    }
}
