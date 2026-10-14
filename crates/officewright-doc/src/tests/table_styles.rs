//! Table styles: what they give tables, rows and cells, through their conditional
//! formats for the parts of a table.

use super::*;

/// A document whose styles part holds `styles` and whose body is `body`.
fn open_with(styles: &str, body: &str) -> Document {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let styles = format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#);
    let extra = [
        ("word/_rels/document.xml.rels", rels),
        ("word/styles.xml", styles.as_str()),
    ];
    Document::from_bytes(package(DOCUMENT, &document_xml(body), &extra)).unwrap()
}

/// A table of `rows` rows of `columns` cells, with these table properties.
fn table(table_properties: &str, rows: usize, columns: usize) -> String {
    let cell = "<w:tc><w:p/></w:tc>".repeat(columns);
    let row = format!("<w:tr>{cell}</w:tr>");
    format!(
        "<w:tbl><w:tblPr>{table_properties}</w:tblPr><w:tblGrid/>{}</w:tbl><w:p/>",
        row.repeat(rows)
    )
}

fn shading(fill: &str) -> String {
    format!(r#"<w:tcPr><w:shd w:val="clear" w:fill="{fill}"/></w:tcPr>"#)
}

/// The first table of the body.
fn first_table(document: &Document) -> Table<'_> {
    document
        .sections()
        .flat_map(|section| section.blocks())
        .find_map(|block| match block {
            Block::Table(table) => Some(table),
            _ => None,
        })
        .unwrap()
}

/// The fill of each cell of a table in effect, row by row: `-` for none.
fn fills(document: &Document) -> Vec<Vec<String>> {
    let mut formats = document.formats();
    first_table(document)
        .rows()
        .map(|row| {
            row.cells()
                .map(|cell| {
                    match formats
                        .cell_properties(cell)
                        .shading()
                        .and_then(|shading| shading.fill().value())
                    {
                        Some(ColorValue::Rgb(red, green, blue)) => {
                            format!("{red:02X}{green:02X}{blue:02X}")
                        }
                        _ => "-".to_owned(),
                    }
                })
                .collect()
        })
        .collect()
}

/// The table styles of the tests: Base gives the whole table, Grid is based on it and
/// gives the parts of a table.
fn grid_styles() -> String {
    [
        format!(
            r#"<w:style w:type="table" w:styleId="Base"><w:name w:val="Base"/><w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:jc w:val="center"/><w:tblW w:w="5000" w:type="dxa"/><w:tblLayout w:type="fixed"/><w:tblBorders><w:top w:val="single" w:sz="4"/></w:tblBorders></w:tblPr><w:trPr><w:cantSplit/></w:trPr>{}</w:style>"#,
            shading("AAAAAA")
        ),
        format!(
            r#"<w:style w:type="table" w:styleId="Grid"><w:name w:val="Grid"/><w:basedOn w:val="Base"/><w:tblStylePr w:type="wholeTable"><w:tcPr><w:noWrap/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstRow"><w:trPr><w:tblHeader/></w:trPr>{}</w:tblStylePr><w:tblStylePr w:type="firstRow">{}</w:tblStylePr><w:tblStylePr w:type="band1Horz">{}</w:tblStylePr><w:tblStylePr w:type="band2Horz">{}</w:tblStylePr><w:tblStylePr w:type="lastCol"><w:tcPr><w:vAlign w:val="center"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="neCell">{}</w:tblStylePr><w:tblStylePr w:type="firstCol">{}</w:tblStylePr></w:style>"#,
            shading("FF0000"),
            shading("999999"),
            shading("00FF00"),
            shading("0000FF"),
            shading("111111"),
            shading("222222"),
        ),
    ]
    .concat()
}

#[test]
fn applies_a_table_styles_conditional_formats_in_words_order() {
    let body = table(
        r#"<w:tblStyle w:val="Grid"/><w:tblInd w:w="360" w:type="dxa"/><w:tblLook w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="1" w:noHBand="0" w:noVBand="1"/>"#,
        4,
        3,
    );
    let document = open_with(&grid_styles(), &body);
    let mut formats = document.formats();
    let table = first_table(&document);

    // The style's table properties, the table's own over them; Word does not allow the
    // width and layout in a style (MS-OI29500 §17.7.6.4(a)).
    let properties = formats.table_properties(table);
    assert_eq!(properties.justification(), Some(TableJustification::Center));
    assert_eq!(properties.indentation(), Some(TableWidth::Length(Pt(18.0))));
    assert_eq!((properties.width(), properties.layout()), (None, None));
    assert!(properties.borders().and_then(TableBorders::top).is_some());
    assert_eq!(properties.row_band_size(), Some(1));

    // The first row (the first of two such formats counting), the bands of the rows after
    // it, starting with the odd one, and the top right corner, as the first row and the
    // last column are on. The first column is off: its format and the top left are not
    // taken.
    assert_eq!(
        fills(&document),
        [
            ["FF0000", "FF0000", "111111"],
            ["00FF00", "00FF00", "00FF00"],
            ["0000FF", "0000FF", "0000FF"],
            ["00FF00", "00FF00", "00FF00"],
        ]
    );
    let rows: Vec<TableRow<'_>> = table.rows().collect();
    let cells: Vec<TableCell<'_>> = rows[0].cells().collect();
    assert_eq!(
        formats.cell_conditions(cells[2]),
        [
            TableStyleCondition::LastColumn,
            TableStyleCondition::FirstRow,
            TableStyleCondition::TopRightCell,
        ]
    );
    // The whole table's format and the last column's.
    let last = formats.cell_properties(cells[2]);
    assert_eq!(last.no_wrap(), Some(true));
    assert_eq!(
        last.vertical_justification(),
        Some(VerticalJustification::Center)
    );
    assert_eq!(
        formats.cell_properties(cells[0]).vertical_justification(),
        None
    );
    // Rows: the style's, and the first row's format for the first row.
    let first = formats.row_properties(rows[0]);
    assert_eq!(
        (first.cant_split(), first.header()),
        (Some(true), Some(true))
    );
    assert_eq!(formats.row_properties(rows[1]).header(), None);
    // One pass over the table for all its rows and cells.
    assert_eq!(formats.work().tables, 1);
}

#[test]
fn takes_words_options_when_a_table_has_no_look() {
    // Without w:tblLook, 0x04A0: the first row and the first column, no vertical bands
    // (MS-OI29500 §17.4.55(a)); the row bands start after the first row.
    let document = open_with(
        &grid_styles(),
        &table(r#"<w:tblStyle w:val="Grid"/>"#, 3, 3),
    );
    let look = document.formats().table_look(first_table(&document));
    assert_eq!(
        (
            look.first_row(),
            look.last_row(),
            look.first_column(),
            look.last_column(),
            look.no_horizontal_bands(),
            look.no_vertical_bands()
        ),
        (
            Some(true),
            Some(false),
            Some(true),
            Some(false),
            Some(false),
            Some(true)
        )
    );
    // The first column's format over the bands; the first row's over the first column's.
    assert_eq!(
        fills(&document),
        [
            ["FF0000", "FF0000", "FF0000"],
            ["222222", "00FF00", "00FF00"],
            ["222222", "0000FF", "0000FF"],
        ]
    );
}

#[test]
fn bands_rows_by_the_band_size_and_skips_the_total_row() {
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Bands"><w:name w:val="Bands"/><w:tblPr><w:tblStyleRowBandSize w:val="2"/></w:tblPr><w:tblStylePr w:type="band1Horz">{}</w:tblStylePr><w:tblStylePr w:type="band2Horz">{}</w:tblStylePr></w:style>"#,
        shading("00FF00"),
        shading("0000FF")
    );
    let look =
        r#"<w:tblStyle w:val="Bands"/><w:tblLook w:firstRow="0" w:lastRow="1" w:noHBand="0"/>"#;
    let document = open_with(&styles, &table(look, 6, 1));
    // Bands of two rows from the first; the total row out of them.
    assert_eq!(
        fills(&document),
        [
            ["00FF00"],
            ["00FF00"],
            ["0000FF"],
            ["0000FF"],
            ["00FF00"],
            ["-"]
        ]
    );
    // Without a band size, Word bands nothing (MS-OI29500 §17.7.6.7).
    let styles = styles.replace(r#"<w:tblStyleRowBandSize w:val="2"/>"#, "");
    let document = open_with(&styles, &table(look, 3, 1));
    assert_eq!(fills(&document), [["-"], ["-"], ["-"]]);
    // noHBand turns them off.
    let styles_with_size = styles.replace(
        "<w:tblPr></w:tblPr>",
        r#"<w:tblPr><w:tblStyleRowBandSize w:val="1"/></w:tblPr>"#,
    );
    let document = open_with(
        &styles_with_size,
        &table(
            r#"<w:tblStyle w:val="Bands"/><w:tblLook w:noHBand="1"/>"#,
            2,
            1,
        ),
    );
    assert_eq!(fills(&document), [["-"], ["-"]]);
}

#[test]
fn applies_the_default_table_style_and_ignores_table_normal() {
    let styles = format!(
        r#"<w:style w:type="table" w:default="1" w:styleId="Plain"><w:name w:val="Plain"/>{}</w:style><w:style w:type="table" w:styleId="TableNormal"><w:name w:val="Normal Table"/>{}</w:style>"#,
        shading("ABCDEF"),
        shading("123456")
    );
    // A table without w:tblStyle has the default table style.
    let document = open_with(&styles, &table("", 1, 1));
    assert_eq!(fills(&document), [["ABCDEF"]]);
    // Word ignores what TableNormal gives (MS-OI29500 §17.7.4.17(a)).
    let document = open_with(
        &styles,
        &table(r#"<w:tblStyle w:val="TableNormal"/>"#, 1, 1),
    );
    assert_eq!(fills(&document), [["-"]]);
    // The cell's own shading over the style's.
    let body = r#"<w:tbl><w:tblPr/><w:tblGrid/><w:tr><w:tc><w:tcPr><w:shd w:val="clear" w:fill="FEDCBA"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_with(&styles, body);
    assert_eq!(fills(&document), [["FEDCBA"]]);
}

#[test]
fn lays_the_rows_and_cells_own_properties_and_partial_looks_over_the_style() {
    let styles = r#"<w:style w:type="table" w:styleId="Lines"><w:name w:val="Lines"/><w:trPr><w:cantSplit/></w:trPr><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="4"/></w:tcBorders></w:tcPr><w:tblStylePr w:type="firstRow"><w:tblPr><w:tblStyleRowBandSize w:val="1"/></w:tblPr><w:tcPr><w:tcBorders><w:bottom w:val="double" w:sz="4"/></w:tcBorders></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstCol"><w:tcPr><w:noWrap/></w:tcPr></w:tblStylePr></w:style>"#;
    // A w:tblLook that gives only the first row: the options it leaves out are off.
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Lines"/><w:tblLook w:firstRow="1"/></w:tblPr><w:tblGrid/><w:tr><w:trPr><w:cantSplit w:val="0"/></w:trPr><w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_with(styles, body);
    let mut formats = document.formats();
    let table = first_table(&document);
    assert_eq!(formats.table_look(table).first_column(), Some(false));
    let rows: Vec<TableRow<'_>> = table.rows().collect();
    // The row's own over the style's.
    assert_eq!(formats.row_properties(rows[0]).cant_split(), Some(false));
    assert_eq!(formats.row_properties(rows[1]).cant_split(), Some(true));
    // Borders side by side: the style's top and the first row's bottom.
    let cell = rows[0].cells().next().unwrap();
    let properties = formats.cell_properties(cell);
    let borders = properties.borders().unwrap().sides();
    assert!(borders.top().is_some() && borders.bottom().is_some());
    // The first column is off, so its format is not taken.
    assert_eq!(properties.no_wrap(), None);
    // A band size in a conditional format, which Word does not allow (MS-OI29500
    // §17.7.6.3(a)), is no table property of it.
    let translation = document.to_layout_ir();
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}
