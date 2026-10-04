//! The calc chain — `xl/calcChain.xml`.
//!
//! Excel keeps a calc chain listing every formula cell so recalculation can
//! walk it in stored order. The chain is **derived state**: Excel rebuilds
//! it happily when the part is absent, and a stale one is worse than none.
//! This module derives it from the workbook ([`CalcChain::from_workbook`])
//! so `write_xlsx` can emit a fresh, consistent chain — cells grouped per
//! sheet index ascending, coordinates ascending within a sheet — and the
//! reader simply tolerates the part's presence (its content is never
//! trusted over the sheets themselves).

use crate::{SheetData, XlsxWorkbook};

/// One entry of the calc chain: a formula cell and its sheet index
/// (1-based, matching `i=` attributes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcEntry {
    /// The cell reference in `A1` notation (`"B3"`).
    pub reference: String,
    /// 1-based index of the sheet holding the cell.
    pub sheet_index: u32,
}

/// The derived calc chain of a workbook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalcChain {
    /// Formula cells, sheet-major then coordinate-ascending.
    pub entries: Vec<CalcEntry>,
}

impl CalcChain {
    /// Derives the chain from `workbook`: every cell with a formula, sorted
    /// by sheet index then by `(row, col)`.
    #[must_use]
    pub fn from_workbook(workbook: &XlsxWorkbook) -> Self {
        let mut entries = Vec::new();
        for (sheet_pos, sheet) in workbook.sheets.iter().enumerate() {
            entries.extend(entries_for_sheet(sheet, sheet_pos));
        }
        CalcChain { entries }
    }

    /// Number of formula cells in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the workbook has no formula cells.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Calc-chain entries for one sheet (0-based position), coordinates ascending.
fn entries_for_sheet(
    sheet: &SheetData,
    sheet_pos: usize,
) -> impl Iterator<Item = CalcEntry> + use<'_> {
    let sheet_index = u32::try_from(sheet_pos + 1).unwrap_or(u32::MAX);
    sheet
        .cells
        .iter()
        .filter(|(_, cell)| cell.formula.is_some())
        .map(move |(&(row, col), _)| CalcEntry {
            reference: sheet_core::refs::cell_name((row, col)),
            sheet_index,
        })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::CalcChain;
    use crate::{SheetData, XlsxCell, XlsxValue, XlsxWorkbook};

    #[test]
    fn chain_is_sheet_major_and_coordinate_sorted() {
        let mut s1 = SheetData::new("A");
        s1.cells
            .insert((3, 0), XlsxCell::formula("A1", XlsxValue::Number(1.0)));
        s1.cells
            .insert((1, 0), XlsxCell::formula("A1", XlsxValue::Number(1.0)));
        s1.cells
            .insert((0, 0), XlsxCell::new(XlsxValue::Number(9.0))); // constant

        let mut s2 = SheetData::new("B");
        s2.cells.insert(
            (0, 0),
            XlsxCell::formula("Z1", XlsxValue::FormulaString(String::new())),
        );

        let wb = XlsxWorkbook {
            sheets: vec![s1, s2],
            ..XlsxWorkbook::default()
        };
        let chain = CalcChain::from_workbook(&wb);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain.entries[0].reference, "A2"); // (1,0) → A2
        assert_eq!(chain.entries[0].sheet_index, 1);
        assert_eq!(chain.entries[1].reference, "A4"); // (3,0) → A4
        assert_eq!(chain.entries[2].reference, "A1");
        assert_eq!(chain.entries[2].sheet_index, 2);
        assert!(!chain.is_empty());

        let empty = XlsxWorkbook::default();
        assert!(CalcChain::from_workbook(&empty).is_empty());
    }

    #[test]
    fn btreemap_iterates_row_major_into_the_chain() {
        let mut sheet = SheetData::new("S");
        sheet
            .cells
            .insert((0, 1), XlsxCell::formula("B1", XlsxValue::Number(0.0)));
        sheet
            .cells
            .insert((0, 0), XlsxCell::formula("A1", XlsxValue::Number(0.0)));
        let order: std::vec::Vec<(u32, u32)> = sheet.cells.keys().copied().collect();
        assert_eq!(order, vec![(0, 0), (0, 1)]);
        let wb = XlsxWorkbook {
            sheets: vec![sheet],
            ..XlsxWorkbook::default()
        };
        let chain = CalcChain::from_workbook(&wb);
        assert_eq!(chain.entries[0].reference, "A1");
        assert_eq!(chain.entries[1].reference, "B1");
    }
}
