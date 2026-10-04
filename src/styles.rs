//! Style-table metadata.
//!
//! v0.1 models the style table as a **count**: cell style *indices*
//! (`s="3"` attributes, [`sheet_xlsx::XlsxCell::style_index`]) survive
//! round-trips, but the definitions behind them (fonts, fills, borders,
//! number formats) are not parsed and a canonical minimal `styles.xml` is
//! emitted on write with `count` `cellXfs` entries. Index-stable, not
//! definition-stable — see the crate docs.

/// The style table as the codec models it: how many `cellXfs` entries exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Styles {
    /// Number of cell-format entries in `xl/styles.xml` (`cellXfs/@count`).
    /// Indices below this count are valid in `s=` attributes.
    pub cell_xfs: u32,
}

impl Styles {
    /// A table with `cell_xfs` entries.
    #[must_use]
    pub const fn with_count(cell_xfs: u32) -> Self {
        Styles { cell_xfs }
    }
}

#[cfg(test)]
mod tests {
    use super::Styles;

    #[test]
    fn count_constructor() {
        assert_eq!(Styles::default().cell_xfs, 0);
        assert_eq!(Styles::with_count(7).cell_xfs, 7);
    }
}
