//! Package writing — `write_xlsx`.
//!
//! Emits the canonical minimal `SpreadsheetML` package: content types, root
//! relationships, `xl/workbook.xml`, per-sheet worksheet parts, the
//! shared-string table, a canonical minimal `styles.xml` (see [`crate::styles`]),
//! and a freshly derived `xl/calcChain.xml` whenever any cell carries a
//! formula ([`crate::calc_chain`]). All text is XML-escaped; output is
//! deterministic — same workbook in, byte-identical package out.

use crate::calc_chain::CalcChain;
use crate::error::XlsxError;
use crate::{SheetData, XlsxValue, XlsxWorkbook};
use quick_xml::events::BytesText;
use quick_xml::Writer;
use sheet_core::refs::cell_name;
use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;
use std::string::String;
use std::vec::Vec;

/// `SpreadsheetML` main namespace.
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
/// Package relationships namespace.
const NS_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// Office document relationship type base.
const NS_OFFICE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Serializes `workbook` into XLSX bytes.
///
/// # Errors
///
/// - [`XlsxError::NonFiniteNumber`] for `NaN`/infinite cell numbers.
/// - [`XlsxError::SharedStringRange`] when a cell's shared-string index
///   points past [`XlsxWorkbook::shared_strings`].
/// - [`XlsxError::Zip`] if container serialization fails.
pub fn write_xlsx(workbook: &XlsxWorkbook) -> Result<Vec<u8>, XlsxError> {
    validate(workbook)?;

    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    // Default compression is Deflated (the `deflate` feature is on).
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();

    let sheet_count = workbook.sheets.len();
    let has_formulas = workbook
        .sheets
        .iter()
        .any(|s| s.cells.values().any(|c| c.formula.is_some()));
    // rId allocation: 1..=N sheets, then styles, then sharedStrings, then calcChain.
    let rid_styles = sheet_count + 1;
    let rid_shared = sheet_count + 2;
    let rid_calc = sheet_count + 3;

    let mut add = |name: &str, bytes: &[u8]| -> Result<(), XlsxError> {
        zip.start_file(name, options).map_err(XlsxError::Zip)?;
        zip.write_all(bytes)
            .map_err(|e| XlsxError::Zip(zip::result::ZipError::Io(e)))
    };

    add(
        "[Content_Types].xml",
        &content_types(sheet_count, has_formulas),
    )?;
    add("_rels/.rels", &root_rels())?;
    add(
        "xl/workbook.xml",
        &workbook_xml(workbook, rid_styles, rid_shared, rid_calc, has_formulas),
    )?;
    add(
        "xl/_rels/workbook.xml.rels",
        &workbook_rels(sheet_count, rid_styles, rid_shared, rid_calc, has_formulas),
    )?;
    for (pos, sheet) in workbook.sheets.iter().enumerate() {
        add(&sheet_path(pos), &worksheet_xml(sheet, workbook)?)?;
    }
    add("xl/styles.xml", &styles_xml(workbook.styles))?;
    add("xl/sharedStrings.xml", &shared_strings_xml(workbook))?;
    if has_formulas {
        add(
            "xl/calcChain.xml",
            &calc_chain_xml(&CalcChain::from_workbook(workbook)),
        )?;
    }

    let finished = zip.finish().map_err(XlsxError::Zip)?;
    Ok(finished.into_inner())
}

/// Cross-checks the workbook before serialization.
fn validate(workbook: &XlsxWorkbook) -> Result<(), XlsxError> {
    for sheet in &workbook.sheets {
        for (coord, cell) in &sheet.cells {
            if let XlsxValue::Number(n) = cell.value {
                if !n.is_finite() {
                    return Err(XlsxError::NonFiniteNumber(n));
                }
            }
            if let XlsxValue::SharedString(index) = cell.value {
                if usize::try_from(index).is_ok_and(|i| i >= workbook.shared_strings.len()) {
                    return Err(XlsxError::SharedStringRange {
                        index,
                        len: workbook.shared_strings.len(),
                    });
                }
            }
            let _ = coord; // coordinates are total (u32, u32)
        }
    }
    Ok(())
}

/// `xl/worksheets/sheetN.xml` for a 0-based sheet position (package path).
fn sheet_path(pos: usize) -> String {
    format!("xl/worksheets/sheet{}.xml", pos + 1)
}

/// Worksheet target relative to the workbook part's directory (`xl/`).
fn sheet_rel_target(pos: usize) -> String {
    format!("worksheets/sheet{}.xml", pos + 1)
}

/// Serializes an XML element with text content into a fresh buffer.
fn element(name: &str, attrs: &[(&str, &str)], content: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf);
    let mut el = w.create_element(name);
    for (k, v) in attrs {
        el = el.with_attribute((*k, *v));
    }
    let _ = el.write_text_content(BytesText::from_escaped(quick_xml::escape::escape(content)));
    buf
}

/// `[Content_Types].xml`.
fn content_types(sheet_count: usize, has_formulas: bool) -> Vec<u8> {
    let mut overrides = String::new();
    overrides.push_str(
        &element(
            "Override",
            &[
                ("PartName", "/xl/workbook.xml"),
                (
                    "ContentType",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                ),
            ],
            "",
        )
        .lossy_string(),
    );
    for pos in 0..sheet_count {
        overrides.push_str(
            &element(
                "Override",
                &[
                    ("PartName", &format!("/{}", sheet_path(pos))),
                    (
                        "ContentType",
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml",
                    ),
                ],
                "",
            )
            .lossy_string(),
        );
    }
    overrides.push_str(
        &element(
            "Override",
            &[
                ("PartName", "/xl/styles.xml"),
                (
                    "ContentType",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml",
                ),
            ],
            "",
        )
        .lossy_string(),
    );
    overrides.push_str(
        &element(
            "Override",
            &[
                ("PartName", "/xl/sharedStrings.xml"),
                (
                    "ContentType",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml",
                ),
            ],
            "",
        )
        .lossy_string(),
    );
    if has_formulas {
        overrides.push_str(
            &element(
                "Override",
                &[
                    ("PartName", "/xl/calcChain.xml"),
                    (
                        "ContentType",
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml",
                    ),
                ],
                "",
            )
            .lossy_string(),
        );
    }

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    out.push_str("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
    out.push_str(
        "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    );
    out.push_str("<Default Extension=\"xml\" ContentType=\"application/xml\"/>");
    out.push_str(&overrides);
    out.push_str("</Types>");
    out.into_bytes()
}

/// `_rels/.rels` — the package-level office-document pointer.
fn root_rels() -> Vec<u8> {
    let rel = element(
        "Relationship",
        &[
            ("Id", "rId1"),
            ("Type", &format!("{NS_OFFICE_REL}/{OFFICE_DOCUMENT}")),
            ("Target", "xl/workbook.xml"),
        ],
        "",
    )
    .lossy_string();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(out, "<Relationships xmlns=\"{NS_RELS}\">");
    out.push_str(&rel);
    out.push_str("</Relationships>");
    out.into_bytes()
}

/// Relationship-type local names.
const OFFICE_DOCUMENT: &str = "officeDocument";
const WORKSHEET: &str = "worksheet";
const SHARED_STRINGS: &str = "sharedStrings";
const STYLES: &str = "styles";
const CALC_CHAIN: &str = "calcChain";

/// `xl/workbook.xml`.
fn workbook_xml(
    workbook: &XlsxWorkbook,
    rid_styles: usize,
    rid_shared: usize,
    rid_calc: usize,
    has_formulas: bool,
) -> Vec<u8> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(
        out,
        "<workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_OFFICE_REL}\"><sheets>"
    );
    for (pos, sheet) in workbook.sheets.iter().enumerate() {
        let rid = pos + 1;
        let name_escaped = quick_xml::escape::escape(&sheet.name).into_owned();
        let _ = write!(
            out,
            "<sheet name=\"{name_escaped}\" sheetId=\"{}\" r:id=\"rId{rid}\"/>",
            pos + 1
        );
    }
    out.push_str("</sheets>");
    if has_formulas {
        let _ = (rid_calc, rid_shared, rid_styles); // rIds are allocated uniformly
        out.push_str("<calcPr calcId=\"0\"/>");
    }
    out.push_str("</workbook>");
    out.into_bytes()
}

/// `xl/_rels/workbook.xml.rels`.
fn workbook_rels(
    sheet_count: usize,
    rid_styles: usize,
    rid_shared: usize,
    rid_calc: usize,
    has_formulas: bool,
) -> Vec<u8> {
    let mut body = String::new();
    for pos in 0..sheet_count {
        let rid = pos + 1;
        body.push_str(
            &element(
                "Relationship",
                &[
                    ("Id", &format!("rId{rid}")),
                    ("Type", &format!("{NS_OFFICE_REL}/{WORKSHEET}")),
                    ("Target", &sheet_rel_target(pos)),
                ],
                "",
            )
            .lossy_string(),
        );
    }
    body.push_str(
        &element(
            "Relationship",
            &[
                ("Id", &format!("rId{rid_styles}")),
                ("Type", &format!("{NS_OFFICE_REL}/{STYLES}")),
                ("Target", "styles.xml"),
            ],
            "",
        )
        .lossy_string(),
    );
    body.push_str(
        &element(
            "Relationship",
            &[
                ("Id", &format!("rId{rid_shared}")),
                ("Type", &format!("{NS_OFFICE_REL}/{SHARED_STRINGS}")),
                ("Target", "sharedStrings.xml"),
            ],
            "",
        )
        .lossy_string(),
    );
    if has_formulas {
        body.push_str(
            &element(
                "Relationship",
                &[
                    ("Id", &format!("rId{rid_calc}")),
                    ("Type", &format!("{NS_OFFICE_REL}/{CALC_CHAIN}")),
                    ("Target", "calcChain.xml"),
                ],
                "",
            )
            .lossy_string(),
        );
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(out, "<Relationships xmlns=\"{NS_RELS}\">");
    out.push_str(&body);
    out.push_str("</Relationships>");
    out.into_bytes()
}

/// Number formatting for `<v>`: shortest round-trip decimal, no exponent
/// padding, integral values without a fractional part.
fn format_number(n: f64) -> String {
    // The 1e15 bound guarantees the f64→i64 cast is lossless (2^53 ≈ 9e15),
    // so the truncation cannot happen by construction.
    #[allow(clippy::cast_possible_truncation)]
    if n.fract() == 0.0_f64 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// One worksheet part.
fn worksheet_xml(sheet: &SheetData, workbook: &XlsxWorkbook) -> Result<Vec<u8>, XlsxError> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(out, "<worksheet xmlns=\"{NS_MAIN}\">");
    if let Some(((min_row, min_col), (max_row, max_col))) = used_range_of(sheet) {
        let _ = write!(
            out,
            "<dimension ref=\"{}:{}\"/>",
            cell_name((min_row, min_col)),
            cell_name((max_row, max_col))
        );
    }
    out.push_str("<sheetData>");

    // Cells iterate row-major; group consecutive same-row cells.
    let mut row_open = false;
    let mut current_row: Option<u32> = None;
    for (&(row, col), cell) in &sheet.cells {
        if current_row != Some(row) {
            if row_open {
                out.push_str("</row>");
            }
            let _ = write!(out, "<row r=\"{}\">", row + 1);
            row_open = true;
            current_row = Some(row);
        }
        out.push_str(&cell_xml(row, col, cell, workbook)?);
    }
    if row_open {
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    Ok(out.into_bytes())
}

/// Bounding box of present cells (`sheet_core`-style).
fn used_range_of(sheet: &SheetData) -> Option<((u32, u32), (u32, u32))> {
    let mut min = (u32::MAX, u32::MAX);
    let mut max = (0u32, 0u32);
    for &(r, c) in sheet.cells.keys() {
        min = (min.0.min(r), min.1.min(c));
        max = (max.0.max(r), max.1.max(c));
    }
    sheet.cells.keys().next().map(|_| (min, max))
}

/// One `<c>` element (no row context).
fn cell_xml(
    row: u32,
    col: u32,
    cell: &crate::XlsxCell,
    workbook: &XlsxWorkbook,
) -> Result<String, XlsxError> {
    let reference = cell_name((row, col));
    let mut attrs = String::from("r=\"");
    attrs.push_str(&reference);
    attrs.push('"');
    if let Some(style) = cell.style_index {
        let _ = write!(attrs, " s=\"{style}\"");
    }

    let body = match &cell.value {
        XlsxValue::Number(n) => {
            if !n.is_finite() {
                return Err(XlsxError::NonFiniteNumber(*n));
            }
            format!("<v>{}</v>", format_number(*n))
        }
        XlsxValue::SharedString(index) => {
            if usize::try_from(*index).is_ok_and(|i| i >= workbook.shared_strings.len()) {
                return Err(XlsxError::SharedStringRange {
                    index: *index,
                    len: workbook.shared_strings.len(),
                });
            }
            attrs.push_str(" t=\"s\"");
            format!("<v>{index}</v>")
        }
        XlsxValue::InlineString(text) => {
            attrs.push_str(" t=\"inlineStr\"");
            let escaped = quick_xml::escape::escape(text);
            format!("<is><t xml:space=\"preserve\">{escaped}</t></is>")
        }
        XlsxValue::Boolean(b) => {
            attrs.push_str(" t=\"b\"");
            format!("<v>{}</v>", u8::from(*b))
        }
        XlsxValue::Error(text) => {
            attrs.push_str(" t=\"e\"");
            let escaped = quick_xml::escape::escape(text);
            format!("<v>{escaped}</v>")
        }
        XlsxValue::FormulaString(text) => {
            attrs.push_str(" t=\"str\"");
            let escaped = quick_xml::escape::escape(text);
            format!("<v>{escaped}</v>")
        }
    };

    let formula = match &cell.formula {
        Some(f) => {
            let escaped = quick_xml::escape::escape(f);
            format!("<f>{escaped}</f>")
        }
        None => String::new(),
    };

    Ok(format!("<c {attrs}>{formula}{body}</c>"))
}

/// Canonical minimal `styles.xml` with the modeled `cellXfs` count. A count
/// of zero is schema-valid (`count` is optional and unsigned); faithfulness
/// beats Excel's usual ≥1 convention here, so round-trips are exact.
fn styles_xml(styles: crate::Styles) -> Vec<u8> {
    let count = styles.cell_xfs;
    let mut xfs = String::new();
    for _ in 0..count {
        xfs.push_str("<xf fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>");
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(out, "<styleSheet xmlns=\"{NS_MAIN}\">");
    out.push_str("<fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>");
    out.push_str("<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>");
    out.push_str("<borders count=\"1\"><border/></borders>");
    out.push_str(
        "<cellStyleXfs count=\"1\"><xf fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>",
    );
    let _ = write!(out, "<cellXfs count=\"{count}\">{xfs}</cellXfs>");
    out.push_str("</styleSheet>");
    out.into_bytes()
}

/// `xl/sharedStrings.xml`.
fn shared_strings_xml(workbook: &XlsxWorkbook) -> Vec<u8> {
    let count = workbook.shared_strings.len();
    let mut items = String::new();
    for text in workbook.shared_strings.iter() {
        let escaped = quick_xml::escape::escape(text);
        let _ = write!(items, "<si><t xml:space=\"preserve\">{escaped}</t></si>");
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(
        out,
        "<sst xmlns=\"{NS_MAIN}\" count=\"{count}\" uniqueCount=\"{count}\">"
    );
    out.push_str(&items);
    out.push_str("</sst>");
    out.into_bytes()
}

/// `xl/calcChain.xml` from the derived chain.
fn calc_chain_xml(chain: &CalcChain) -> Vec<u8> {
    let mut entries = String::new();
    for entry in &chain.entries {
        let _ = write!(
            entries,
            "<c r=\"{}\" i=\"{}\"/>",
            entry.reference, entry.sheet_index
        );
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    let _ = write!(out, "<calcChain xmlns=\"{NS_MAIN}\">");
    out.push_str(&entries);
    out.push_str("</calcChain>");
    out.into_bytes()
}

/// Helper: `Vec<u8>` → lossy `String` for embedding pre-rendered elements.
trait LossyString {
    fn lossy_string(&self) -> String;
}

impl LossyString for Vec<u8> {
    fn lossy_string(&self) -> String {
        String::from_utf8_lossy(self).into_owned()
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

    use super::{format_number, sheet_path};

    #[test]
    fn number_formatting() {
        assert_eq!(format_number(41.0), "41");
        assert_eq!(format_number(-7.0), "-7");
        assert_eq!(format_number(1.5), "1.5");
        assert_eq!(format_number(0.1), "0.1");
        assert_eq!(format_number(1e20), "100000000000000000000");
    }

    #[test]
    fn sheet_paths_are_one_based() {
        assert_eq!(sheet_path(0), "xl/worksheets/sheet1.xml");
        assert_eq!(sheet_path(9), "xl/worksheets/sheet10.xml");
    }
}
