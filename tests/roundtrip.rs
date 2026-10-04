//! Round-trip and golden tests.
//!
//! The **golden reader** tests zip hand-written XML parts (independent of
//! the writer code path) and assert the exact parsed model — this is the
//! check that the reader understands `SpreadsheetML`, not just its own
//! output. Round-trip tests then pin write→read equality for every cell
//! type, and the error tests cover the typed-failure surface.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

use sheet_xlsx::{
    read_xlsx, write_xlsx, CalcChain, SheetData, Styles, XlsxCell, XlsxError, XlsxValue,
    XlsxWorkbook,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::vec::Vec;

/// Fixture workbook exercising every cell type.
fn fixture() -> XlsxWorkbook {
    let mut cells = BTreeMap::new();
    cells.insert((0, 0), XlsxCell::new(XlsxValue::Number(41.5)));
    cells.insert((0, 1), XlsxCell::new(XlsxValue::SharedString(0)));
    cells.insert(
        (0, 2),
        XlsxCell::new(XlsxValue::InlineString("in <line>".to_string())),
    );
    cells.insert((1, 0), XlsxCell::new(XlsxValue::Boolean(true)));
    cells.insert((1, 1), XlsxCell::new(XlsxValue::Error("#REF!".to_string())));
    cells.insert((1, 2), XlsxCell::formula("B1+1", XlsxValue::Number(42.5)));
    cells.insert(
        (2, 0),
        XlsxCell::formula(
            "CONCAT(A1, \"x\")",
            XlsxValue::FormulaString("41.5x".to_string()),
        ),
    );
    cells.insert(
        (2, 1),
        XlsxCell {
            value: XlsxValue::Number(7.0),
            style_index: Some(3),
            formula: None,
        },
    );
    let mut wb = XlsxWorkbook {
        sheets: vec![SheetData {
            name: String::from("Data"),
            cells,
        }],
        ..XlsxWorkbook::default()
    };
    wb.shared_strings.insert("shared text");
    wb.styles = Styles::with_count(4);
    wb
}

#[test]
fn roundtrip_preserves_every_cell_exactly() {
    let wb = fixture();
    let bytes = write_xlsx(&wb).unwrap();
    let back = read_xlsx(&bytes).unwrap();

    assert_eq!(back, wb, "round trip must be exact");
    assert_eq!(back.sheets[0].name, "Data");
    // Spot-check the interesting cells for failure messages that help.
    assert_eq!(
        back.sheets[0]
            .cells
            .get(&(1, 2))
            .unwrap()
            .formula
            .as_deref(),
        Some("B1+1")
    );
    assert_eq!(
        back.sheets[0].cells.get(&(0, 1)).unwrap().value,
        XlsxValue::SharedString(0)
    );
    assert_eq!(back.shared_text(0), Some("shared text"));
    assert_eq!(back.styles.cell_xfs, 4);
}

#[test]
fn xml_special_characters_survive() {
    let mut cells = BTreeMap::new();
    let nasty = "<tag> & \"quotes\" 'apostrophes' éж✓ ]]>";
    cells.insert(
        (0, 0),
        XlsxCell::new(XlsxValue::InlineString(nasty.to_string())),
    );
    let mut wb = XlsxWorkbook::default();
    wb.sheets.push(SheetData {
        name: "<S & T>".to_string(),
        cells,
    });

    let bytes = write_xlsx(&wb).unwrap();
    let back = read_xlsx(&bytes).unwrap();
    assert_eq!(
        back.sheets[0].cells.get(&(0, 0)).unwrap().value,
        XlsxValue::InlineString(nasty.to_string())
    );
    assert_eq!(back.sheets[0].name, "<S & T>");
}

#[test]
fn shared_string_dedup_on_write() {
    let mut cells = BTreeMap::new();
    cells.insert(
        (0, 0),
        XlsxCell::new(XlsxValue::InlineString("same".to_string())),
    );
    cells.insert(
        (0, 1),
        XlsxCell::new(XlsxValue::InlineString("same".to_string())),
    );
    let mut wb = XlsxWorkbook::default();
    wb.sheets.push(SheetData {
        name: "S".to_string(),
        cells,
    });

    let bytes = write_xlsx(&wb).unwrap();
    let back = read_xlsx(&bytes).unwrap();
    // Inline strings stay inline (faithful variant round-trip); reading the
    // written package's shared table stays empty.
    assert!(back.shared_strings.is_empty());
    assert_eq!(
        back.sheets[0].cells.get(&(0, 1)).unwrap().value,
        XlsxValue::InlineString("same".to_string())
    );
}

#[test]
fn calc_chain_is_emitted_only_with_formulas() {
    let with = write_xlsx(&fixture()).unwrap();
    let raw = std::string::String::from_utf8_lossy(&with).to_string();
    assert!(raw.contains("calcChain"), "fixture has formulas");

    let mut plain = XlsxWorkbook::default();
    plain.sheets.push(SheetData::new("Sheet1"));
    let without = write_xlsx(&plain).unwrap();
    let raw = std::string::String::from_utf8_lossy(&without).to_string();
    assert!(!raw.contains("calcChain"));
}

#[test]
fn derived_calc_chain_matches_cells() {
    let wb = fixture();
    let chain = CalcChain::from_workbook(&wb);
    assert_eq!(chain.len(), 2);
    // Row-major order: (1,2) sorts before (2,0).
    assert_eq!(chain.entries[0].reference, "C2"); // (1,2)
    assert_eq!(chain.entries[0].sheet_index, 1);
    assert_eq!(chain.entries[1].reference, "A3"); // (2,0)
}

/// Zips hand-written parts (no writer involvement) into package bytes.
fn handcrafted_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    for (name, xml) in parts {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

/// A golden package with: rich-text shared strings, an inlineStr cell, a
/// boolean, an error, a formula with cached number, a style-only marker we
/// expect to be dropped, and a cell with no `r` attribute.
#[test]
fn golden_reader_understands_handwritten_xml() {
    let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Golden" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
    let shared = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2">
<si><t>plain</t></si>
<si><r><t>rich </t></r><r><t xml:space="preserve"> runs &amp; all</t></r></si>
</sst>"#;
    let styles = r#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<fonts count="1"/><fills count="1"/><borders count="1"/>
<cellStyleXfs count="1"><xf/></cellStyleXfs>
<cellXfs count="2"><xf xfId="0"/><xf xfId="0"/></cellXfs>
</styleSheet>"#;
    let sheet = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<dimension ref="A1:C4"/>
<sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>inline &lt;text&gt;</t></is></c></row>
<row r="2"><c r="A2" t="b"><v>1</v></c><c r="B2" t="e"><v>#DIV/0!</v></c><c r="C2"><f>A1&amp;1</f><v>5</v></c></row>
<row r="3"><c r="A3" s="1"/><c t="str"><f>Z9</f><v>cached</v></c></row>
</sheetData>
</worksheet>"#;

    let bytes = handcrafted_package(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels),
        ("xl/sharedStrings.xml", shared),
        ("xl/styles.xml", styles),
        ("xl/worksheets/sheet1.xml", sheet),
    ]);
    let wb = read_xlsx(&bytes).unwrap();

    assert_eq!(wb.sheets.len(), 1);
    let sheet = &wb.sheets[0];
    assert_eq!(sheet.name, "Golden");
    assert_eq!(wb.shared_strings.len(), 2);
    assert_eq!(wb.shared_text(0), Some("plain"));
    assert_eq!(wb.shared_text(1), Some("rich  runs & all"));
    assert_eq!(wb.styles.cell_xfs, 2);

    let get = |r: u32, c: u32| sheet.cells.get(&(r, c));
    assert_eq!(get(0, 0).unwrap().value, XlsxValue::SharedString(0));
    assert_eq!(get(0, 1).unwrap().value, XlsxValue::SharedString(1));
    assert_eq!(
        get(0, 2).unwrap().value,
        XlsxValue::InlineString("inline <text>".to_string())
    );
    assert_eq!(get(1, 0).unwrap().value, XlsxValue::Boolean(true));
    assert_eq!(
        get(1, 1).unwrap().value,
        XlsxValue::Error("#DIV/0!".to_string())
    );
    let formula_cell = get(1, 2).unwrap();
    assert_eq!(formula_cell.formula.as_deref(), Some("A1&1"));
    assert_eq!(formula_cell.value, XlsxValue::Number(5.0));

    // Style-only cell (no value, no formula) is dropped; the `r`-less cell
    // lands at (row, cursor) = (2, 1).
    assert!(get(2, 0).is_none());
    let rless = get(2, 1).unwrap();
    assert_eq!(rless.value, XlsxValue::FormulaString("cached".to_string()));
    assert_eq!(rless.formula.as_deref(), Some("Z9"));
}

/// The reader tolerates a calcChain part and absolute `/xl/...` targets.
#[test]
fn reader_tolerates_calc_chain_and_absolute_targets() {
    let workbook = r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="T" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let rels = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="/xl/worksheets/sheet1.xml"/>
</Relationships>"#;
    let sheet = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="A1"><f>1+1</f></c></row></sheetData></worksheet>"#;
    let calc = r#"<?xml version="1.0"?>
<calcChain xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><c r="A1" i="1"/></calcChain>"#;

    let bytes = handcrafted_package(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/calcChain.xml", calc),
    ]);
    let wb = read_xlsx(&bytes).unwrap();
    let cell = wb.sheets[0].cells.get(&(0, 0)).unwrap();
    // Formula with no cached value → the documented empty-string marker.
    assert_eq!(cell.formula.as_deref(), Some("1+1"));
    assert_eq!(cell.value, XlsxValue::FormulaString(String::new()));
}

/// Repeated sheets: names and order follow workbook.xml.
#[test]
fn multiple_sheets_keep_order() {
    let wb = fixture();
    let mut second = SheetData::new("Extra");
    second
        .cells
        .insert((0, 0), XlsxCell::new(XlsxValue::Number(2.0)));
    let mut wb = wb;
    wb.sheets.push(second);

    let bytes = write_xlsx(&wb).unwrap();
    let back = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets.len(), 2);
    assert_eq!(back.sheets[0].name, "Data");
    assert_eq!(back.sheets[1].name, "Extra");
    assert_eq!(
        back.sheets[1].cells.get(&(0, 0)).unwrap().value,
        XlsxValue::Number(2.0)
    );
}

#[test]
fn typed_errors_on_broken_input() {
    // Not a zip.
    let err = read_xlsx(b"not a zip at all").unwrap_err();
    assert!(matches!(err, XlsxError::Zip(_)), "{err:?}");

    // Zip without the workbook part.
    let empty = handcrafted_package(&[("[Content_Types].xml", CONTENT_TYPES)]);
    let err = read_xlsx(&empty).unwrap_err();
    assert!(matches!(err, XlsxError::MissingPart { .. }), "{err:?}");

    // Corrupt cell reference.
    let workbook = r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="T" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let rels = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#;
    let sheet = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="3B"><v>1</v></c></row></sheetData></worksheet>"#;
    let bytes = handcrafted_package(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels),
        ("xl/worksheets/sheet1.xml", sheet),
    ]);
    let err = read_xlsx(&bytes).unwrap_err();
    assert!(
        matches!(&err, XlsxError::CorruptCell { reference, .. } if reference == "3B"),
        "{err:?}"
    );

    // Unparseable number in a numeric cell.
    let sheet = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="A1"><v>abc</v></c></row></sheetData></worksheet>"#;
    let bytes = handcrafted_package(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels),
        ("xl/worksheets/sheet1.xml", sheet),
    ]);
    let err = read_xlsx(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            XlsxError::CorruptCell {
                reason: "unparseable number",
                ..
            }
        ),
        "{err:?}"
    );

    // Dangling relationship id.
    let sheet_ok = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#;
    let bad_rels = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;
    let bytes = handcrafted_package(&[
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", bad_rels),
        ("xl/worksheets/sheet1.xml", sheet_ok),
    ]);
    let err = read_xlsx(&bytes).unwrap_err();
    assert!(
        matches!(&err, XlsxError::MissingRelationship { id } if id == "rId1"),
        "{err:?}"
    );
}

#[test]
fn write_refuses_non_finite_numbers_and_bad_shared_indices() {
    let mut wb = XlsxWorkbook::default();
    let mut sheet = SheetData::new("S");
    sheet
        .cells
        .insert((0, 0), XlsxCell::new(XlsxValue::Number(f64::NAN)));
    wb.sheets.push(sheet);
    let err = write_xlsx(&wb).unwrap_err();
    assert!(matches!(err, XlsxError::NonFiniteNumber(_)), "{err:?}");

    let mut wb = XlsxWorkbook::default();
    let mut sheet = SheetData::new("S");
    sheet
        .cells
        .insert((0, 0), XlsxCell::new(XlsxValue::SharedString(4)));
    wb.sheets.push(sheet);
    let err = write_xlsx(&wb).unwrap_err();
    assert!(
        matches!(err, XlsxError::SharedStringRange { index: 4, len: 0 }),
        "{err:?}"
    );
}

#[test]
fn output_is_deterministic() {
    let a = write_xlsx(&fixture()).unwrap();
    let b = write_xlsx(&fixture()).unwrap();
    assert_eq!(a, b, "same workbook in, byte-identical package out");
}
