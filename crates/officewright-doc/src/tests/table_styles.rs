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
    // The last column's; Word ignores the whole table's format (MS-OI29500
    // §17.18.89(a)), so its no wrapping is not taken.
    let last = formats.cell_properties(cells[2]);
    assert_eq!(last.no_wrap(), None);
    assert_eq!(
        last.vertical_justification(),
        Some(VerticalJustification::Center)
    );
    // The last column's vertical alignment applies to whole rows (MS-OI29500
    // §17.7.6.8(a)), its other properties not.
    let first_cell = formats.cell_properties(cells[0]);
    assert_eq!(
        first_cell.vertical_justification(),
        Some(VerticalJustification::Center)
    );
    assert_eq!(
        first_cell
            .shading()
            .and_then(|shading| shading.fill().value()),
        Some(ColorValue::Rgb(0xFF, 0, 0))
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
    // Word does not allow table-wide cell borders or a width in the style's own cell
    // properties (MS-OI29500 §17.7.6.9); the whole table's conditional format may give
    // borders.
    let styles = r#"<w:style w:type="table" w:styleId="Lines"><w:name w:val="Lines"/><w:trPr><w:cantSplit/></w:trPr><w:tcPr><w:tcW w:w="500" w:type="dxa"/><w:tcBorders><w:start w:val="single" w:sz="4"/></w:tcBorders></w:tcPr><w:tblStylePr w:type="wholeTable"><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="4"/></w:tcBorders></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstRow"><w:tblPr><w:tblStyleRowBandSize w:val="1"/></w:tblPr><w:tcPr><w:tcBorders><w:bottom w:val="double" w:sz="4"/></w:tcBorders></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstCol"><w:tcPr><w:noWrap/></w:tcPr></w:tblStylePr></w:style>"#;
    // A w:tblLook that gives only the first row: the options it leaves out are on, as
    // Word's render of word-rules/E-table-style-look.docx, case 06, shows (#300).
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Lines"/><w:tblLook w:firstRow="1"/></w:tblPr><w:tblGrid/><w:tr><w:trPr><w:cantSplit w:val="0"/></w:trPr><w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_with(styles, body);
    let mut formats = document.formats();
    let table = first_table(&document);
    assert_eq!(formats.table_look(table).first_column(), Some(true));
    let rows: Vec<TableRow<'_>> = table.rows().collect();
    // The row's own over the style's.
    assert_eq!(formats.row_properties(rows[0]).cant_split(), Some(false));
    assert_eq!(formats.row_properties(rows[1]).cant_split(), Some(true));
    // The first row's bottom border; the whole table's top border is not taken, as Word
    // ignores whole-table formats (MS-OI29500 §17.18.89(a)).
    let cell = rows[0].cells().next().unwrap();
    let properties = formats.cell_properties(cell);
    let borders = properties.borders().unwrap().sides();
    assert!(borders.top().is_none() && borders.bottom().is_some());
    assert!(borders.start().is_none());
    assert_eq!(properties.width(), None);
    // The first column is on, so its format is taken.
    assert_eq!(properties.no_wrap(), Some(true));
    // A band size in a conditional format, which Word does not allow (MS-OI29500
    // §17.7.6.3(a)), is no table property of it.
    let translation = document.to_layout_ir();
    assert!(
        translation.left_out().is_empty(),
        "{:?}",
        translation.left_out()
    );
}

#[test]
fn ignores_whole_table_formats_and_turns_on_what_a_look_does_not_give() {
    // Two whole-table formats, which Word ignores (MS-OI29500 §17.18.89(a)), and a span
    // in the style's own cell properties, which Word does not allow (§17.7.6.8(b)).
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Twice"><w:name w:val="Twice"/><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:tblStylePr w:type="wholeTable">{}</w:tblStylePr><w:tblStylePr w:type="wholeTable">{}</w:tblStylePr><w:tblStylePr w:type="firstRow">{}</w:tblStylePr></w:style>"#,
        shading("123456"),
        shading("654321"),
        shading("FF0000")
    );
    // A w:tblLook that gives only the last column: the first row it leaves out is on
    // (word-rules/E-table-style-look.docx, case 06, #300).
    let body = table(
        r#"<w:tblStyle w:val="Twice"/><w:tblLook w:lastColumn="1"/>"#,
        2,
        1,
    );
    let document = open_with(&styles, &body);
    let mut formats = document.formats();
    let styled = first_table(&document);
    assert_eq!(formats.table_look(styled).first_row(), Some(true));
    assert_eq!(fills(&document), [["FF0000"], ["-"]]);
    let cell = styled.rows().next().unwrap().cells().next().unwrap();
    assert_eq!(formats.cell_properties(cell).grid_span(), None);
}

/// The cell at `row` and `column` of the first table.
fn cell_at(document: &Document, row: usize, column: usize) -> TableCell<'_> {
    first_table(document)
        .rows()
        .nth(row)
        .and_then(|row| row.cells().nth(column))
        .unwrap()
}

/// The widths in points of a cell's top, bottom, start and end borders in effect.
fn side_widths<'a>(formats: &mut Formats<'a>, cell: TableCell<'a>) -> [Option<f64>; 4] {
    let properties = formats.cell_properties(cell);
    let width = |side: Option<&Border>| side.and_then(Border::width).map(|width| width.0);
    match properties.borders() {
        Some(borders) => {
            let sides = borders.sides();
            [
                width(sides.top()),
                width(sides.bottom()),
                width(sides.start()),
                width(sides.end()),
            ]
        }
        None => [None; 4],
    }
}

/// `w:tcBorders` whose sides tell themselves apart by their width: top 1 pt, bottom
/// 1.5 pt, start 2 pt, end 2.5 pt, inside horizontal 3 pt, inside vertical 3.5 pt.
fn group_borders() -> &'static str {
    r#"<w:tcPr><w:tcBorders><w:top w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="12"/><w:start w:val="single" w:sz="16"/><w:end w:val="single" w:sz="20"/><w:insideH w:val="single" w:sz="24"/><w:insideV w:val="single" w:sz="28"/></w:tcBorders></w:tcPr>"#
}

#[test]
fn draws_a_conditional_format_s_borders_around_and_inside_its_group_of_cells() {
    // The borders of a conditional format belong to its group of cells: the outer ones
    // on the group's edges, w:insideH and w:insideV on the edges inside it (ISO/IEC
    // 29500-1, insideH and insideV of tcBorders).
    let (top, bottom, start, end, inside_h, inside_v) = (
        Some(1.0),
        Some(1.5),
        Some(2.0),
        Some(2.5),
        Some(3.0),
        Some(3.5),
    );
    let style = |kind: &str| {
        format!(
            r#"<w:style w:type="table" w:styleId="Group"><w:name w:val="Group"/><w:tblPr><w:tblStyleRowBandSize w:val="2"/><w:tblStyleColBandSize w:val="2"/></w:tblPr><w:tblStylePr w:type="{kind}">{}</w:tblStylePr></w:style>"#,
            group_borders()
        )
    };
    let look = |look: &str| format!(r#"<w:tblStyle w:val="Group"/><w:tblLook {look}/>"#);
    let none = r#"w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1""#;

    // The first row: one row of three cells.
    let document = open_with(
        &style("firstRow"),
        &table(
            &look(&none.replace(r#"w:firstRow="0""#, r#"w:firstRow="1""#)),
            2,
            3,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [top, bottom, start, inside_v]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 1)),
        [top, bottom, inside_v, inside_v]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 2)),
        [top, bottom, inside_v, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 0)),
        [None; 4]
    );

    // The first column: one column of three rows.
    let document = open_with(
        &style("firstCol"),
        &table(
            &look(&none.replace(r#"w:firstColumn="0""#, r#"w:firstColumn="1""#)),
            3,
            2,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [top, inside_h, start, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 0)),
        [inside_h, inside_h, start, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 2, 0)),
        [inside_h, bottom, start, end]
    );

    // The last column and the last row are groups too.
    let document = open_with(
        &style("lastCol"),
        &table(
            &look(&none.replace(r#"w:lastColumn="0""#, r#"w:lastColumn="1""#)),
            3,
            2,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 1)),
        [top, inside_h, start, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 2, 1)),
        [inside_h, bottom, start, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 0)),
        [None; 4]
    );
    let document = open_with(
        &style("lastRow"),
        &table(
            &look(&none.replace(r#"w:lastRow="0""#, r#"w:lastRow="1""#)),
            2,
            3,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 0)),
        [top, bottom, start, inside_v]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 2)),
        [top, bottom, inside_v, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [None; 4]
    );

    // A band of two rows, and the band after it cut short by the table's end.
    let document = open_with(
        &style("band1Horz"),
        &table(
            &look(&none.replace(r#"w:noHBand="1""#, r#"w:noHBand="0""#)),
            5,
            2,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [top, inside_h, start, inside_v]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 1)),
        [inside_h, bottom, inside_v, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 2, 0)),
        [None; 4]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 4, 0)),
        [top, bottom, start, inside_v]
    );

    // A band of two columns, over every row.
    let document = open_with(
        &style("band1Vert"),
        &table(
            &look(&none.replace(r#"w:noVBand="1""#, r#"w:noVBand="0""#)),
            2,
            3,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [top, inside_h, start, inside_v]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 1, 1)),
        [inside_h, bottom, inside_v, end]
    );
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 2)),
        [None; 4]
    );

    // A corner is a group of its own.
    let document = open_with(
        &style("nwCell"),
        &table(
            &look(
                &none
                    .replace(r#"w:firstRow="0""#, r#"w:firstRow="1""#)
                    .replace(r#"w:firstColumn="0""#, r#"w:firstColumn="1""#),
            ),
            2,
            2,
        ),
    );
    let mut formats = document.formats();
    assert_eq!(
        side_widths(&mut formats, cell_at(&document, 0, 0)),
        [top, bottom, start, end]
    );
}

#[test]
fn keeps_the_style_s_border_where_a_cell_sets_its_own_to_none() {
    // A cell's own border of style none takes the table style's, as an omitted one does
    // (MS-OI29500 §17.4.66(a)); nil is no border.
    let styles = r#"<w:style w:type="table" w:styleId="Lined"><w:name w:val="Lined"/><w:tblStylePr w:type="firstRow"><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="8"/></w:tcBorders></w:tcPr></w:tblStylePr><w:tblStylePr w:type="lastCol"><w:tcPr><w:tcBorders><w:start w:val="dotted" w:sz="8"/><w:end w:val="dashed" w:sz="8"/></w:tcBorders></w:tcPr></w:tblStylePr></w:style>"#;
    let cell = |borders: &str| {
        format!(r#"<w:tc><w:tcPr><w:tcBorders>{borders}</w:tcBorders></w:tcPr><w:p/></w:tc>"#)
    };
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Lined"/><w:tblLook w:firstRow="1" w:lastColumn="1"/></w:tblPr><w:tblGrid/><w:tr>{}{}{}</w:tr></w:tbl><w:p/>"#,
        cell(r#"<w:top w:val="none"/><w:bottom w:val="nil"/><w:start w:val="none"/>"#),
        cell(r#"<w:top w:val="double" w:sz="4"/>"#),
        cell(r#"<w:start w:val="none"/><w:end w:val="none"/>"#),
    );
    let document = open_with(styles, &body);
    let mut formats = document.formats();
    let first = formats.cell_properties(cell_at(&document, 0, 0));
    let sides = first.borders().unwrap().sides();
    assert_eq!(
        sides.top().and_then(Border::style),
        Some(BorderStyle::Single)
    );
    assert_eq!(
        sides.bottom().and_then(Border::style),
        Some(BorderStyle::Nil)
    );
    // Where the style has no border, the cell's none stays.
    assert_eq!(
        sides.start().and_then(Border::style),
        Some(BorderStyle::None)
    );
    let second = formats.cell_properties(cell_at(&document, 0, 1));
    assert_eq!(
        second
            .borders()
            .unwrap()
            .sides()
            .top()
            .and_then(Border::style),
        Some(BorderStyle::Double)
    );
    // The start and end sides too, from the last column's format.
    let third = formats.cell_properties(cell_at(&document, 0, 2));
    let sides = third.borders().unwrap().sides();
    assert_eq!(
        sides.start().and_then(Border::style),
        Some(BorderStyle::Dotted)
    );
    assert_eq!(
        sides.end().and_then(Border::style),
        Some(BorderStyle::Dashed)
    );
}

#[test]
fn lays_a_derived_style_s_format_over_its_base_s_of_the_same_type() {
    // The first row's format of a style based on another adds to the base's: the
    // derived shading and the base's border.
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Base"><w:name w:val="Base"/><w:tblStylePr w:type="firstRow"><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="8"/></w:tcBorders><w:shd w:val="clear" w:fill="111111"/></w:tcPr></w:tblStylePr></w:style><w:style w:type="table" w:styleId="Derived"><w:name w:val="Derived"/><w:basedOn w:val="Base"/><w:tblStylePr w:type="firstRow">{}</w:tblStylePr></w:style>"#,
        shading("222222")
    );
    let document = open_with(
        &styles,
        &table(
            r#"<w:tblStyle w:val="Derived"/><w:tblLook w:firstRow="1"/>"#,
            2,
            1,
        ),
    );
    assert_eq!(fills(&document), [["222222"], ["-"]]);
    let mut formats = document.formats();
    let first = formats.cell_properties(cell_at(&document, 0, 0));
    assert!(
        first
            .borders()
            .and_then(|borders| borders.sides().top())
            .is_some()
    );
}

#[test]
fn leaves_out_the_row_properties_word_does_not_take_from_a_style() {
    // Word does not allow the grid and widths before and after the cells or the height
    // in a table style's rows, and ignores w:hidden there (MS-OI29500 §17.7.6.10,
    // §17.7.6.11); a row's own height stays.
    let row = r#"<w:trPr><w:gridBefore w:val="2"/><w:gridAfter w:val="1"/><w:wBefore w:w="2000" w:type="dxa"/><w:wAfter w:w="1000" w:type="dxa"/><w:trHeight w:val="2000" w:hRule="exact"/><w:hidden/><w:cantSplit/></w:trPr>"#;
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Rows"><w:name w:val="Rows"/>{row}<w:tblStylePr w:type="firstRow">{}</w:tblStylePr></w:style>"#,
        row.replace("<w:cantSplit/>", "<w:tblHeader/>")
    );
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Rows"/><w:tblLook w:firstRow="1"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc></w:tr><w:tr><w:trPr><w:trHeight w:val="400"/></w:trPr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_with(&styles, body);
    let mut formats = document.formats();
    let rows: Vec<TableRow<'_>> = first_table(&document).rows().collect();
    let first = formats.row_properties(rows[0]);
    assert_eq!(
        (
            first.grid_before(),
            first.grid_after(),
            first.width_before(),
            first.width_after()
        ),
        (None, None, None, None)
    );
    assert_eq!((first.height(), first.hidden()), (None, None));
    assert_eq!(
        (first.cant_split(), first.header()),
        (Some(true), Some(true))
    );
    let second = formats.row_properties(rows[1]);
    assert_eq!(second.height().and_then(RowHeight::height), Some(Pt(20.0)));
    assert_eq!(second.header(), None);
    assert!(
        document.to_layout_ir().left_out().is_empty(),
        "{:?}",
        document.to_layout_ir().left_out()
    );
}

#[test]
fn bands_by_the_style_s_band_size_even_above_three() {
    // A table's own band size is not allowed (MS-OI29500 §17.4.59(a)). Word bands by a
    // style's size of 5, though §17.7.6.7(c) keeps it within 0 to 3:
    // word-rules/E-table-style-bands-5.docx (#300).
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Wide"><w:name w:val="Wide"/><w:tblPr><w:tblStyleRowBandSize w:val="5"/></w:tblPr><w:tblStylePr w:type="band1Horz">{}</w:tblStylePr><w:tblStylePr w:type="band2Horz">{}</w:tblStylePr></w:style>"#,
        shading("111111"),
        shading("222222")
    );
    let body = table(
        r#"<w:tblStyle w:val="Wide"/><w:tblStyleRowBandSize w:val="1"/><w:tblLook w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#,
        8,
        1,
    );
    let document = open_with(&styles, &body);
    let column: Vec<String> = fills(&document).into_iter().flatten().collect();
    assert_eq!(
        column,
        [
            "111111", "111111", "111111", "111111", "111111", "222222", "222222", "222222"
        ]
    );
    let mut formats = document.formats();
    assert_eq!(
        formats
            .table_properties(first_table(&document))
            .row_band_size(),
        Some(5)
    );
}

#[test]
fn applies_the_formats_of_one_cell_in_word_s_order() {
    let style = |formats: &[(&str, &str)]| {
        let parts: String = formats
            .iter()
            .map(|(kind, fill)| {
                format!(
                    r#"<w:tblStylePr w:type="{kind}">{}</w:tblStylePr>"#,
                    shading(fill)
                )
            })
            .collect();
        format!(
            r#"<w:style w:type="table" w:styleId="Order"><w:name w:val="Order"/><w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:tblStyleColBandSize w:val="1"/></w:tblPr>{parts}</w:style>"#
        )
    };
    let fill = |styles: &str, look: &str, rows: usize, columns: usize| {
        let body = table(
            &format!(r#"<w:tblStyle w:val="Order"/><w:tblLook {look}/>"#),
            rows,
            columns,
        );
        fills(&open_with(styles, &body))
    };
    // Row bands, then column bands (MS-OI29500 §17.7.6.6(a)).
    let bands = style(&[("band1Vert", "0000AA"), ("band1Horz", "AA0000")]);
    assert_eq!(
        fill(
            &bands,
            r#"w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="0""#,
            1,
            1
        ),
        [["0000AA"]]
    );
    // The first column, then the last.
    let columns = style(&[("lastCol", "BBBBBB"), ("firstCol", "AAAAAA")]);
    assert_eq!(
        fill(
            &columns,
            r#"w:firstRow="0" w:firstColumn="1" w:lastColumn="1" w:noHBand="1""#,
            1,
            1
        ),
        [["BBBBBB"]]
    );
    // The first row, then the last; only the table's last row is its last.
    let rows = style(&[("lastRow", "BBBBBB"), ("firstRow", "AAAAAA")]);
    assert_eq!(
        fill(&rows, r#"w:firstRow="0" w:lastRow="1" w:noHBand="1""#, 3, 1),
        [["-"], ["-"], ["BBBBBB"]]
    );
    // The top left corner, then the top right.
    let corners = style(&[("neCell", "BBBBBB"), ("nwCell", "AAAAAA")]);
    assert_eq!(
        fill(
            &corners,
            r#"w:firstRow="1" w:firstColumn="1" w:lastColumn="1" w:noHBand="1""#,
            1,
            1
        ),
        [["BBBBBB"]]
    );
    // Column bands leave out the first and last column when those are on.
    let banded = style(&[("band1Vert", "111111"), ("band2Vert", "222222")]);
    assert_eq!(
        fill(
            &banded,
            r#"w:firstRow="0" w:firstColumn="1" w:lastColumn="1" w:noHBand="1" w:noVBand="0""#,
            1,
            5
        ),
        [["-", "111111", "222222", "111111", "-"]]
    );
}

#[test]
fn takes_what_a_format_gives_whole_rows_and_leaves_out_what_a_style_may_not_give() {
    // The first column's no wrapping, margins and vertical alignment reach every cell of
    // its rows; its shading, text direction and diagonal border only its own cells.
    let styles = r#"<w:style w:type="table" w:styleId="Column"><w:name w:val="Column"/><w:tblPr><w:jc w:val="center"/><w:tblpPr w:tblpX="100" w:tblpY="100"/></w:tblPr><w:tcPr><w:textDirection w:val="tbRl"/></w:tcPr><w:tblStylePr w:type="firstCol"><w:tcPr><w:tcBorders><w:tl2br w:val="single" w:sz="4"/></w:tcBorders><w:shd w:val="clear" w:fill="AAAAAA"/><w:noWrap/><w:tcMar><w:top w:w="100" w:type="dxa"/></w:tcMar><w:vAlign w:val="bottom"/></w:tcPr></w:tblStylePr></w:style><w:style w:type="table" w:styleId="Derived"><w:name w:val="Derived"/><w:basedOn w:val="Column"/></w:style>"#;
    let body = table(
        r#"<w:tblStyle w:val="Derived"/><w:tblLook w:firstRow="0" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/>"#,
        2,
        2,
    );
    let document = open_with(styles, &body);
    let mut formats = document.formats();
    let first = formats.cell_properties(cell_at(&document, 1, 0));
    assert!(
        first
            .borders()
            .unwrap()
            .top_left_to_bottom_right()
            .is_some()
    );
    assert!(first.shading().is_some());
    assert_eq!(first.text_direction(), None);
    let second = formats.cell_properties(cell_at(&document, 1, 1));
    assert_eq!(second.no_wrap(), Some(true));
    assert!(second.margins().and_then(CellMargins::top).is_some());
    assert_eq!(
        second.vertical_justification(),
        Some(VerticalJustification::Bottom)
    );
    assert!(second.shading().is_none() && second.borders().is_none());
    // Through the style it is based on, the table takes the alignment, not the position.
    let properties = formats.table_properties(first_table(&document));
    assert_eq!(properties.justification(), Some(TableJustification::Center));
    assert!(properties.position().is_none());
    // A cell's conditions are its own, not its row's.
    assert_eq!(
        formats.cell_conditions(cell_at(&document, 0, 0)),
        [TableStyleCondition::FirstColumn]
    );
    assert!(formats.cell_conditions(cell_at(&document, 0, 1)).is_empty());
}

#[test]
fn takes_the_default_table_style_and_text_a_style_gives_through_a_format_or_a_base() {
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="First"><w:name w:val="First"/>{}</w:style><w:style w:type="table" w:default="1" w:styleId="Chosen"><w:name w:val="Chosen"/>{}</w:style><w:style w:type="table" w:styleId="Bold"><w:name w:val="Bold"/><w:rPr><w:b/></w:rPr></w:style><w:style w:type="table" w:styleId="OnBold"><w:name w:val="OnBold"/><w:basedOn w:val="Bold"/></w:style><w:style w:type="table" w:styleId="Header"><w:name w:val="Header"/><w:tblStylePr w:type="firstRow"><w:rPr><w:b/></w:rPr></w:tblStylePr></w:style>"#,
        shading("111111"),
        shading("222222")
    );
    // Without w:tblStyle, the default table style.
    let document = open_with(&styles, &table("", 1, 1));
    assert_eq!(fills(&document), [["222222"]]);
    // Text in a whole-table format, which Word ignores with the rest of it, is no text
    // the style gives.
    let whole = r#"<w:style w:type="table" w:styleId="Whole"><w:name w:val="Whole"/><w:tblStylePr w:type="wholeTable"><w:rPr><w:b/></w:rPr></w:tblStylePr></w:style>"#;
    let document = open_with(whole, &table(r#"<w:tblStyle w:val="Whole"/>"#, 1, 1));
    assert_eq!(marks(&document), [(11.0, false, ColorValue::Auto)]);
    assert!(document.to_layout_ir().left_out().is_empty());
    // Through the style it is based on, and through the first row's format.
    for id in ["OnBold", "Header"] {
        let document = open_with(
            &styles,
            &table(&format!(r#"<w:tblStyle w:val="{id}"/>"#), 2, 1),
        );
        let bold: Vec<bool> = marks(&document).iter().map(|mark| mark.1).collect();
        assert_eq!(bold, [true, id == "OnBold"], "{id}");
        assert!(document.to_layout_ir().left_out().is_empty(), "{id}");
    }
}

/// A style "Parts" that shades each part of a table with a fill of its own, banding rows
/// and columns by one.
fn parts_style() -> String {
    let parts: String = [
        ("band1Horz", "111111"),
        ("band2Horz", "222222"),
        ("band1Vert", "333333"),
        ("band2Vert", "444444"),
        ("firstCol", "CCCCCC"),
        ("lastCol", "DDDDDD"),
        ("firstRow", "AAAAAA"),
        ("lastRow", "BBBBBB"),
    ]
    .iter()
    .map(|(kind, fill)| {
        format!(
            r#"<w:tblStylePr w:type="{kind}">{}</w:tblStylePr>"#,
            shading(fill)
        )
    })
    .collect();
    format!(
        r#"<w:style w:type="table" w:styleId="Parts"><w:name w:val="Parts"/><w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:tblStyleColBandSize w:val="1"/></w:tblPr>{parts}</w:style>"#
    )
}

#[test]
fn takes_the_options_a_partial_look_leaves_out_as_on() {
    // word-rules/E-table-style-look.docx (#300): once one of the six options is written,
    // Word ignores w:val and takes each option not written as on; noHBand and noVBand on
    // mean no bands.
    let fill = |look: &str| {
        fills(&open_with(
            &parts_style(),
            &table(
                &format!(r#"<w:tblStyle w:val="Parts"/><w:tblLook {look}/>"#),
                3,
                3,
            ),
        ))
    };
    // Case 06, only w:firstRow 1: every option on (Word saves it as w:val 07E0).
    assert_eq!(
        fill(r#"w:firstRow="1""#),
        [
            ["AAAAAA", "AAAAAA", "AAAAAA"],
            ["CCCCCC", "-", "DDDDDD"],
            ["BBBBBB", "BBBBBB", "BBBBBB"]
        ]
    );
    // Case 05, w:val 04A0 and w:firstRow 0: the first row off, the rest on (07C0).
    assert_eq!(
        fill(r#"w:val="04A0" w:firstRow="0""#),
        [
            ["CCCCCC", "-", "DDDDDD"],
            ["CCCCCC", "-", "DDDDDD"],
            ["BBBBBB", "BBBBBB", "BBBBBB"]
        ]
    );
    // A w:tblLook with neither w:val nor an option: everything off, so both bands are
    // on, and column bands over row bands (our choice; no render shows it yet).
    assert_eq!(
        fill(""),
        [
            ["333333", "444444", "333333"],
            ["333333", "444444", "333333"],
            ["333333", "444444", "333333"]
        ]
    );
    // Case 04, w:val 04A0 alone: the mask, the first row and column and row bands.
    assert_eq!(
        fill(r#"w:val="04A0""#),
        [
            ["AAAAAA", "AAAAAA", "AAAAAA"],
            ["CCCCCC", "111111", "111111"],
            ["CCCCCC", "222222", "222222"]
        ]
    );
}

#[test]
fn takes_the_first_row_format_for_every_header_row() {
    // word-rules/E-table-style-header-rows-on and -off.docx (#300): with the first row's
    // format on, every header row (w:tblHeader) at the top takes it, and the row bands
    // start after the last of them.
    let rows = |header: &[bool], look: &str| {
        let rows: String = header
            .iter()
            .map(|&header| {
                let tr_pr = if header {
                    "<w:trPr><w:tblHeader/></w:trPr>"
                } else {
                    ""
                };
                format!("<w:tr>{tr_pr}<w:tc><w:p/></w:tc></w:tr>")
            })
            .collect();
        let body = format!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="Parts"/><w:tblLook {look}/></w:tblPr><w:tblGrid/>{rows}</w:tbl><w:p/>"#
        );
        fills(&open_with(&parts_style(), &body))
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    };
    let first_row_and_row_bands = r#"w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1""#;
    assert_eq!(
        rows(
            &[true, true, false, false, false, false],
            first_row_and_row_bands
        ),
        ["AAAAAA", "AAAAAA", "111111", "222222", "111111", "222222"]
    );
    // Only the header rows at the top: one after a row that is not one is a body row
    // (our choice, as Word repeats only those at the top; no render shows it yet, #298).
    assert_eq!(
        rows(&[false, true, false, false], first_row_and_row_bands),
        ["AAAAAA", "111111", "222222", "111111"]
    );
    assert_eq!(
        rows(&[true, false, true, false], first_row_and_row_bands),
        ["AAAAAA", "111111", "222222", "111111"]
    );
    // With the first row's format off, header rows are banded like the rest (our choice;
    // no render shows it yet, #298).
    let row_bands = r#"w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1""#;
    assert_eq!(
        rows(&[true, true, false, false], row_bands),
        ["111111", "222222", "111111", "222222"]
    );
}

#[test]
fn makes_the_header_rows_one_group_of_the_first_row_s_format() {
    // Our choice, which no render shows yet (#298): the header rows are one group for the
    // first row's borders, so only the first takes its top border and only the last its
    // bottom one, with w:insideH between them; and each takes the corner formats.
    let styles = r#"<w:style w:type="table" w:styleId="Head"><w:name w:val="Head"/><w:tblStylePr w:type="firstRow"><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="16"/><w:insideH w:val="single" w:sz="4"/></w:tcBorders></w:tcPr></w:tblStylePr><w:tblStylePr w:type="nwCell"><w:tcPr><w:shd w:val="clear" w:fill="ABCDEF"/></w:tcPr></w:tblStylePr></w:style>"#;
    let header = "<w:trPr><w:tblHeader/></w:trPr>";
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Head"/><w:tblLook w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid/><w:tr>{header}<w:tc><w:p/></w:tc></w:tr><w:tr>{header}<w:tc><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
    );
    let document = open_with(styles, &body);
    let mut formats = document.formats();
    let widths: Vec<_> = (0..3)
        .map(|row| {
            let [top, bottom, _, _] = side_widths(&mut formats, cell_at(&document, row, 0));
            (top, bottom)
        })
        .collect();
    assert_eq!(
        widths,
        [(Some(1.0), Some(0.5)), (Some(0.5), Some(2.0)), (None, None)]
    );
    assert_eq!(fills(&document), [["ABCDEF"], ["ABCDEF"], ["-"]]);
}

#[test]
fn gives_every_header_row_the_first_row_s_row_properties_and_both_top_corners() {
    // The rest of the choice no render shows yet (#298): every header row takes the first
    // row's row properties and the top right corner as well as the top left.
    let styles = r#"<w:style w:type="table" w:styleId="Head"><w:name w:val="Head"/><w:tblStylePr w:type="firstRow"><w:trPr><w:cantSplit/></w:trPr></w:tblStylePr><w:tblStylePr w:type="nwCell"><w:tcPr><w:shd w:val="clear" w:fill="AAAAAA"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="neCell"><w:tcPr><w:shd w:val="clear" w:fill="BBBBBB"/></w:tcPr></w:tblStylePr></w:style>"#;
    let header = "<w:trPr><w:tblHeader/></w:trPr>";
    let cells = "<w:tc><w:p/></w:tc><w:tc><w:p/></w:tc>";
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Head"/><w:tblLook w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="1" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid/><w:tr>{header}{cells}</w:tr><w:tr>{header}{cells}</w:tr><w:tr>{cells}</w:tr></w:tbl><w:p/>"#
    );
    let document = open_with(styles, &body);
    assert_eq!(
        fills(&document),
        [["AAAAAA", "BBBBBB"], ["AAAAAA", "BBBBBB"], ["-", "-"]]
    );
    let mut formats = document.formats();
    let rows: Vec<TableRow<'_>> = first_table(&document).rows().collect();
    let cant_split: Vec<_> = rows
        .iter()
        .map(|&row| formats.row_properties(row).cant_split())
        .collect();
    assert_eq!(cant_split, [Some(true), Some(true), None]);
}

#[test]
fn takes_the_last_table_style_marked_default() {
    // Of several table styles marked w:default, the last, as the standard says
    // ([MS-WPE376] Part 4 §2.7.3.17 quotes it) and Word shows for paragraph styles (#308).
    let styles = format!(
        r#"<w:style w:type="table" w:default="1" w:styleId="First"><w:name w:val="First"/>{}</w:style><w:style w:type="table" w:default="1" w:styleId="Second"><w:name w:val="Second"/>{}</w:style>"#,
        shading("111111"),
        shading("222222")
    );
    let document = open_with(&styles, &table("", 1, 1));
    assert_eq!(fills(&document), [["222222"]]);
    assert_eq!(
        document
            .styles()
            .default_style(StyleKind::Table)
            .map(Style::id),
        Some("Second")
    );
}

#[test]
fn lays_a_list_level_over_each_table_style_of_its_cell() {
    // From the review of #360: a list item's level laid over a paragraph style is kept
    // per table text, so a list item in a right-aligned table's cell does not take the
    // centring of the list item before it, in a table whose style centres.
    let styles = r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="table" w:styleId="C"><w:name w:val="C"/><w:pPr><w:jc w:val="center"/></w:pPr></w:style><w:style w:type="table" w:styleId="R"><w:name w:val="R"/><w:pPr><w:jc w:val="right"/></w:pPr></w:style>"#;
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let item = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#;
    let table = |style: &str| {
        format!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="{style}"/></w:tblPr><w:tblGrid/><w:tr><w:tc>{item}</w:tc></w:tr></w:tbl><w:p/>"#
        )
    };
    let body = format!("{}{}", table("C"), table("R"));
    let document = open_text(styles, "", numbering, &body);
    let mut formats = document.formats();
    let mut found = Vec::new();
    for block in document.sections().next().unwrap().blocks() {
        let Block::Table(table) = block else {
            continue;
        };
        let cell = table.rows().next().unwrap().cells().next().unwrap();
        let Some(Block::Paragraph(paragraph)) = cell.blocks().next() else {
            panic!("no paragraph");
        };
        let format = formats.paragraph(paragraph);
        assert!(format.list().is_some());
        found.push(format.justification());
    }
    assert_eq!(found, [Justification::Center, Justification::Right]);
}

/// A document with these styles, the settings `settings` (none for an empty string), a
/// numbering part when `numbering` is not empty, and this body.
fn open_text(styles: &str, settings: &str, numbering: &str, body: &str) -> Document {
    let mut parts = vec![(
        "styles",
        "styles.xml",
        format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#),
    )];
    if !settings.is_empty() {
        parts.push((
            "settings",
            "settings.xml",
            format!(r#"<w:settings xmlns:w="{W}">{settings}</w:settings>"#),
        ));
    }
    if !numbering.is_empty() {
        parts.push((
            "numbering",
            "numbering.xml",
            format!(r#"<w:numbering xmlns:w="{W}">{numbering}</w:numbering>"#),
        ));
    }
    with_parts(body, &parts)
}

/// The paragraphs of every cell of the document's first table, row by row, and of the
/// tables in them.
fn cell_paragraphs(document: &Document) -> Vec<Paragraph<'_>> {
    fn walk<'a>(table: Table<'a>, out: &mut Vec<Paragraph<'a>>) {
        for row in table.rows() {
            for cell in row.cells() {
                for block in cell.blocks() {
                    match block {
                        Block::Paragraph(paragraph) => out.push(paragraph),
                        Block::Table(table) => walk(table, out),
                        _ => {}
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(first_table(document), &mut out);
    out
}

/// The size, boldness and colour of each cell paragraph's mark.
fn marks(document: &Document) -> Vec<(f64, bool, ColorValue)> {
    let mut formats = document.formats();
    cell_paragraphs(document)
        .into_iter()
        .map(|paragraph| {
            let mark = formats.paragraph_mark(paragraph);
            (mark.size().0, mark.bold(), mark.color())
        })
        .collect()
}

fn rgb(hex: &str) -> ColorValue {
    let [_, red, green, blue] = u32::from_str_radix(hex, 16).unwrap().to_be_bytes();
    ColorValue::Rgb(red, green, blue)
}

#[test]
fn gives_cells_what_the_table_style_and_its_conditional_formats_give_text() {
    // ECMA-376 Part 1, §17.7.2: what a table style gives paragraphs and runs lies under
    // the paragraph style. Each conditional format gives a colour, laid over the style's
    // own size and centring in Word's order (MS-OI29500 §17.7.6.6(a)), as the cells of
    // word-rules/E-table-style-look.docx show; a paragraph outside the table takes none.
    let colours: String = [
        ("band1Horz", "111111"),
        ("band2Horz", "222222"),
        ("firstCol", "CCCCCC"),
        ("firstRow", "AAAAAA"),
        ("nwCell", "EEEEEE"),
    ]
    .iter()
    .map(|(kind, colour)| {
        // The first row's format right-aligns its text too.
        let p_pr = if *kind == "firstRow" {
            r#"<w:pPr><w:jc w:val="right"/></w:pPr>"#
        } else {
            ""
        };
        format!(
            r#"<w:tblStylePr w:type="{kind}">{p_pr}<w:rPr><w:color w:val="{colour}"/></w:rPr></w:tblStylePr>"#
        )
    })
    .collect();
    let styles = format!(
        r#"<w:style w:type="table" w:styleId="Text"><w:name w:val="Text"/><w:pPr><w:jc w:val="center"/><w:spacing w:after="120"/></w:pPr><w:rPr><w:sz w:val="28"/></w:rPr><w:tblPr><w:tblStyleRowBandSize w:val="1"/></w:tblPr>{colours}</w:style>"#
    );
    let look = r#"<w:tblStyle w:val="Text"/><w:tblLook w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#;
    let body = format!(
        "{}<w:p><w:r><w:t>after</w:t></w:r></w:p>",
        table(look, 3, 2).trim_end_matches("<w:p/>")
    );
    let document = open_text(&styles, "", "", &body);
    let found: Vec<ColorValue> = marks(&document).iter().map(|mark| mark.2).collect();
    assert_eq!(
        found,
        [
            // The first row; its first cell is the top left corner.
            rgb("EEEEEE"),
            rgb("AAAAAA"),
            // The first column over the bands, then the bands.
            rgb("CCCCCC"),
            rgb("111111"),
            rgb("CCCCCC"),
            rgb("222222"),
        ]
    );
    let mut formats = document.formats();
    for (at, paragraph) in cell_paragraphs(&document).into_iter().enumerate() {
        let format = formats.paragraph(paragraph);
        let justification = if at < 2 {
            Justification::Right
        } else {
            Justification::Center
        };
        assert_eq!(format.justification(), justification, "{at}");
        assert_eq!(format.after_spacing(), Pt(6.0));
        assert_eq!(formats.paragraph_mark(paragraph).size(), Pt(14.0));
    }
    let after = document
        .sections()
        .next()
        .unwrap()
        .blocks()
        .find_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        formats.paragraph(after).justification(),
        Justification::Start
    );
    let run = after.runs().next().unwrap();
    // Word's own run defaults, without w:docDefaults.
    assert_eq!(formats.run(run).size(), Pt(11.0));
    assert_eq!(formats.run(run).color(), ColorValue::Auto);
}

#[test]
fn puts_the_table_style_under_the_paragraph_style_the_character_style_and_direct_formatting() {
    let styles = r#"<w:style w:type="table" w:styleId="Big"><w:name w:val="Big"/><w:pPr><w:jc w:val="center"/></w:pPr><w:rPr><w:sz w:val="28"/><w:color w:val="111111"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Huge"><w:name w:val="Huge"/><w:pPr><w:jc w:val="right"/></w:pPr><w:rPr><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="character" w:styleId="Red"><w:name w:val="Red"/><w:rPr><w:color w:val="FF0000"/></w:rPr></w:style>"#;
    let cells = [
        "<w:p><w:r><w:t>x</w:t></w:r></w:p>",
        r#"<w:p><w:pPr><w:pStyle w:val="Huge"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:rPr><w:rStyle w:val="Red"/><w:sz w:val="36"/></w:rPr></w:pPr><w:r><w:rPr><w:rStyle w:val="Red"/><w:sz w:val="36"/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    ];
    let row: String = cells
        .iter()
        .map(|cell| format!("<w:tc>{cell}</w:tc>"))
        .collect();
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Big"/></w:tblPr><w:tblGrid/><w:tr>{row}</w:tr></w:tbl><w:p/>"#
    );
    let document = open_text(styles, "", "", &body);
    assert_eq!(
        marks(&document),
        [
            (14.0, false, rgb("111111")),
            (16.0, false, rgb("111111")),
            (18.0, false, rgb("FF0000")),
        ]
    );
    let mut formats = document.formats();
    // The runs take what their marks take.
    let runs: Vec<(f64, ColorValue)> = cell_paragraphs(&document)
        .into_iter()
        .map(|paragraph| {
            let run = formats.run(paragraph.runs().next().unwrap());
            (run.size().0, run.color())
        })
        .collect();
    assert_eq!(
        runs,
        [
            (14.0, rgb("111111")),
            (16.0, rgb("111111")),
            (18.0, rgb("FF0000"))
        ]
    );
    let alignments: Vec<Justification> = cell_paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.paragraph(paragraph).justification())
        .collect();
    assert_eq!(
        alignments,
        [
            Justification::Center,
            Justification::Right,
            Justification::Center
        ]
    );
}

#[test]
fn flips_toggles_with_the_table_style_as_one_more_level() {
    // word-rules/E-table-style-toggles.docx and E-table-style-toggles-defaults.docx: a
    // table style flips a toggle property as the paragraph and character styles do,
    // each when its value differs from the document defaults'.
    let toggles = |defaults: &str, table: &str| {
        let styles = format!(
            r#"<w:docDefaults><w:rPrDefault><w:rPr>{defaults}</w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="table" w:styleId="T"><w:name w:val="T"/><w:rPr>{table}</w:rPr></w:style><w:style w:type="paragraph" w:styleId="BoldPara"><w:name w:val="BoldPara"/><w:rPr><w:b/></w:rPr></w:style><w:style w:type="character" w:styleId="CharBold"><w:name w:val="CharBold"/><w:rPr><w:b/></w:rPr></w:style><w:style w:type="character" w:styleId="CharBoldOff"><w:name w:val="CharBoldOff"/><w:rPr><w:b w:val="0"/></w:rPr></w:style>"#
        );
        let cells = [
            r#"<w:p><w:pPr><w:pStyle w:val="BoldPara"/></w:pPr></w:p>"#,
            "<w:p/>",
            r#"<w:p><w:pPr><w:rPr><w:rStyle w:val="CharBold"/></w:rPr></w:pPr></w:p>"#,
            r#"<w:p><w:pPr><w:rPr><w:rStyle w:val="CharBoldOff"/></w:rPr></w:pPr></w:p>"#,
        ];
        let row: String = cells
            .iter()
            .map(|cell| format!("<w:tc>{cell}</w:tc>"))
            .collect();
        let body = format!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="T"/></w:tblPr><w:tblGrid/><w:tr>{row}</w:tr></w:tbl><w:p/>"#
        );
        let document = open_text(&styles, "", "", &body);
        marks(&document)
            .into_iter()
            .map(|mark| mark.1)
            .collect::<Vec<_>>()
    };
    // Defaults not bold, a bold table style: BoldPara, Normal, CharBold, CharBoldOff.
    assert_eq!(toggles("", "<w:b/>"), [false, true, false, true]);
    // Bold defaults, a table style that turns bold off.
    assert_eq!(
        toggles("<w:b/>", r#"<w:b w:val="0"/>"#),
        [false, false, false, true]
    );
    // Without a table style's value, the paragraph style sets it, as before.
    assert_eq!(toggles("", ""), [true, false, true, false]);
    // An on/off property that does not toggle takes the last level that gives it: the
    // table style's, unless the paragraph style gives one.
    let styles = r#"<w:style w:type="table" w:styleId="T"><w:name w:val="T"/><w:rPr><w:dstrike/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Off"><w:name w:val="Off"/><w:rPr><w:dstrike w:val="0"/></w:rPr></w:style>"#;
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="T"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc><w:tc><w:p><w:pPr><w:pStyle w:val="Off"/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_text(styles, "", "", body);
    let mut formats = document.formats();
    let struck: Vec<bool> = cell_paragraphs(&document)
        .into_iter()
        .map(|paragraph| formats.paragraph_mark(paragraph).double_strike())
        .collect();
    assert_eq!(struck, [true, false]);
}

#[test]
fn lets_the_table_style_win_over_the_default_style_s_11_or_12_pt_and_left_when_the_setting_is_off()
{
    // [MS-DOCX] overrideTableStyleFontSizeAndJustification: off, a size of 11 or 12 pt
    // and a left justification of the default paragraph style give way to the table
    // style's (word-rules/E-table-style-size-12pt-14.docx); not written, it is off below
    // compatibility mode 15 and on from it (E-table-style-size-12pt-none.docx). The
    // document defaults give 11 pt, as in those documents.
    let case = |normal: &str, mode: u32, setting: &[&str], style: &str| {
        let styles = format!(
            r#"<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/>{normal}</w:style><w:style w:type="paragraph" w:styleId="Child"><w:name w:val="Child"/><w:basedOn w:val="Normal"/></w:style><w:style w:type="table" w:styleId="Big"><w:name w:val="Big"/><w:pPr><w:jc w:val="center"/></w:pPr><w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>"#
        );
        let setting: String = setting
            .iter()
            .map(|value| {
                format!(
                    r#"<w:compatSetting w:name="overrideTableStyleFontSizeAndJustification" w:uri="http://schemas.microsoft.com/office/word" w:val="{value}"/>"#
                )
            })
            .collect();
        let settings = format!(
            r#"<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="{mode}"/>{setting}</w:compat>"#
        );
        let paragraph = if style.is_empty() {
            "<w:p/>".to_owned()
        } else {
            format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr></w:p>"#)
        };
        let body = format!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="Big"/></w:tblPr><w:tblGrid/><w:tr><w:tc>{paragraph}</w:tc></w:tr></w:tbl><w:p/>"#
        );
        let document = open_text(&styles, &settings, "", &body);
        let mut formats = document.formats();
        let paragraph = cell_paragraphs(&document)[0];
        let mark = formats.paragraph_mark(paragraph);
        (
            mark.size().0,
            mark.size_complex_script().0,
            formats.paragraph(paragraph).justification(),
        )
    };
    let left_12 = r#"<w:pPr><w:jc w:val="left"/></w:pPr><w:rPr><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>"#;
    let left_13 = r#"<w:pPr><w:jc w:val="left"/></w:pPr><w:rPr><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr>"#;
    let left_11 = r#"<w:rPr><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr>"#;
    let table_wins = (14.0, 14.0, Justification::Center);
    // Normal's 12 pt overrides the table style's 14 pt. Its `w:jc left` never does: it is
    // the justification below Normal, which Word does not keep as Normal's own (the
    // renders of E-table-style-size-12pt.docx and -13pt.docx, setting on, centre the
    // cells), so the table style's centring shows through.
    let normal_12 = (12.0, 12.0, Justification::Center);
    // Mode 14, the setting not written: off.
    assert_eq!(case(left_12, 14, &[], ""), table_wins);
    assert_eq!(case(left_11, 14, &[], "").0, 14.0);
    // The size and the justification each give way on their own.
    assert_eq!(
        case(left_13, 14, &[], ""),
        (13.0, 13.0, Justification::Center)
    );
    // The paragraph names the default style.
    assert_eq!(case(left_12, 14, &[], "Normal"), table_wins);
    // Our choice, no render shows it yet: a style based on the default style keeps
    // what it inherits, the size; the left justification is not Normal's to inherit.
    assert_eq!(case(left_12, 14, &[], "Child"), normal_12);
    // A size equal to the document defaults' (11 pt) is not Normal's own either: the
    // table style's 14 pt shows through even with the setting on, as the render of
    // E-table-style-size-11pt.docx shows.
    assert_eq!(case(left_11, 15, &["1"], "").0, 14.0);
    // Mode 15, not written: on, the plain hierarchy.
    assert_eq!(case(left_12, 15, &[], ""), normal_12);
    // Written, whatever the mode.
    assert_eq!(case(left_12, 15, &["0"], ""), table_wins);
    assert_eq!(case(left_12, 14, &["1"], ""), normal_12);
    // Written twice, the last counts.
    assert_eq!(case(left_12, 14, &["0", "1"], ""), normal_12);
    assert_eq!(case(left_12, 15, &["1", "0"], ""), table_wins);
    // Strict documents write `start` for `left`.
    let start_12 = r#"<w:pPr><w:jc w:val="start"/></w:pPr><w:rPr><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>"#;
    assert_eq!(case(start_12, 14, &[], ""), table_wins);
    // Only a left justification gives way.
    let both_12 = r#"<w:pPr><w:jc w:val="both"/></w:pPr><w:rPr><w:sz w:val="24"/></w:rPr>"#;
    assert_eq!(case(both_12, 14, &[], "").2, Justification::Both);
    // A table style that gives no size or justification leaves the default style's.
    let plain = |normal: &str| {
        let styles = format!(
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/>{normal}</w:style><w:style w:type="table" w:styleId="Plain"><w:name w:val="Plain"/><w:rPr><w:b/></w:rPr></w:style>"#
        );
        let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Plain"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
        let document = open_text(&styles, "", "", body);
        let mut formats = document.formats();
        let paragraph = cell_paragraphs(&document)[0];
        (
            formats.paragraph_mark(paragraph).size().0,
            formats.paragraph(paragraph).justification(),
        )
    };
    // Normal's `w:jc left` is not its own, so the defaults' justification, start, is in
    // effect: the same alignment.
    assert_eq!(plain(left_12), (12.0, Justification::Start));
}

#[test]
fn takes_the_style_of_the_innermost_table_and_gives_list_labels_its_run_properties() {
    let styles = r#"<w:style w:type="table" w:styleId="Outer"><w:name w:val="Outer"/><w:rPr><w:color w:val="111111"/></w:rPr></w:style><w:style w:type="table" w:styleId="Inner"><w:name w:val="Inner"/><w:rPr><w:color w:val="222222"/></w:rPr></w:style><w:style w:type="table" w:styleId="Listed"><w:name w:val="Listed"/><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let inner = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Inner"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#;
    let item =
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr></w:p>"#;
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Outer"/></w:tblPr><w:tblGrid/><w:tr><w:tc>{inner}{item}</w:tc></w:tr></w:tbl><w:p/>"#
    );
    let document = open_text(styles, "", numbering, &body);
    let found: Vec<ColorValue> = marks(&document).iter().map(|mark| mark.2).collect();
    assert_eq!(found, [rgb("222222"), rgb("111111")]);
    let mut formats = document.formats();
    let label = formats.list_label(cell_paragraphs(&document)[1]).unwrap();
    assert_eq!(label.color(), rgb("111111"));
    // The same list level in a cell of a table whose style centres its text, and outside
    // the table: each takes what lies under it.
    let centred = r#"<w:style w:type="table" w:styleId="Centred"><w:name w:val="Centred"/><w:pPr><w:jc w:val="center"/></w:pPr></w:style>"#;
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Centred"/></w:tblPr><w:tblGrid/><w:tr><w:tc>{item}</w:tc></w:tr></w:tbl>{item}"#
    );
    let document = open_text(centred, "", numbering, &body);
    let mut formats = document.formats();
    let inside = cell_paragraphs(&document)[0];
    let outside = document
        .sections()
        .next()
        .unwrap()
        .blocks()
        .find_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph),
            _ => None,
        })
        .unwrap();
    assert!(formats.paragraph(inside).list().is_some());
    assert_eq!(
        formats.paragraph(inside).justification(),
        Justification::Center
    );
    assert_eq!(
        formats.paragraph(outside).justification(),
        Justification::Start
    );
    // A table style's list is not taken, our choice (numbering lies above table styles).
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Listed"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_text(styles, "", numbering, body);
    let paragraph = cell_paragraphs(&document)[0];
    assert!(document.formats().paragraph(paragraph).list().is_none());
}

#[test]
fn clears_a_table_style_s_tab_stop_with_the_paragraph_style() {
    // A paragraph style's cleared stop removes the one the table style sets, which lies
    // below it.
    let styles = r#"<w:style w:type="table" w:styleId="Tabs"><w:name w:val="Tabs"/><w:pPr><w:tabs><w:tab w:val="left" w:pos="1440"/><w:tab w:val="left" w:pos="2880"/></w:tabs></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Clear"><w:name w:val="Clear"/><w:pPr><w:tabs><w:tab w:val="clear" w:pos="1440"/></w:tabs></w:pPr></w:style>"#;
    let body = r#"<w:tbl><w:tblPr><w:tblStyle w:val="Tabs"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc><w:tc><w:p><w:pPr><w:pStyle w:val="Clear"/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p/>"#;
    let document = open_text(styles, "", "", body);
    let mut formats = document.formats();
    let stops: Vec<Vec<f64>> = cell_paragraphs(&document)
        .into_iter()
        .map(|paragraph| {
            formats
                .paragraph(paragraph)
                .tabs()
                .iter()
                .map(|stop| stop.position().0)
                .collect()
        })
        .collect();
    assert_eq!(stops, [vec![72.0, 144.0], vec![144.0]]);
}

#[test]
fn works_out_what_a_table_style_gives_text_once_per_set_of_conditions() {
    // 2,000 cells of one table meet six sets of conditions; each is worked out once,
    // however many paragraphs and runs ask.
    let styles = parts_style();
    let rows = format!(
        "<w:tr>{}</w:tr>",
        "<w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc>".repeat(20)
    )
    .repeat(100);
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Parts"/><w:tblLook w:firstRow="1" w:lastRow="1" w:firstColumn="1" w:lastColumn="1" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid/>{rows}</w:tbl><w:p/>"#
    );
    let document = open_with(&styles, &body);
    let mut formats = document.formats();
    for paragraph in cell_paragraphs(&document) {
        formats.paragraph(paragraph);
        formats.paragraph_mark(paragraph);
        for run in paragraph.runs() {
            formats.run(run);
        }
    }
    assert!(
        formats.work().table_texts <= 12,
        "{}",
        formats.work().table_texts
    );
    assert!(formats.style_reads() < 20, "{}", formats.style_reads());
}

#[test]
fn keeps_at_most_256_table_texts() {
    // 300 tables, each in a style of its own: the table texts are forgotten when there
    // are 256, and what each cell gets stays right.
    let styles: String = (0..300)
        .map(|n| {
            format!(
                r#"<w:style w:type="table" w:styleId="S{n}"><w:name w:val="S{n}"/><w:pPr><w:jc w:val="{}"/></w:pPr><w:rPr><w:sz w:val="{}"/></w:rPr></w:style>"#,
                if n % 2 == 0 { "center" } else { "right" },
                10 + n % 40
            )
        })
        .collect();
    let body: String = (0..300)
        .map(|n| {
            format!(
                r#"<w:tbl><w:tblPr><w:tblStyle w:val="S{n}"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
            )
        })
        .collect();
    let document = open_with(&styles, &body);
    let mut formats = document.formats();
    let mut n = 0;
    for block in document.sections().next().unwrap().blocks() {
        let Block::Table(table) = block else {
            continue;
        };
        let cell = table.rows().next().unwrap().cells().next().unwrap();
        let Some(Block::Paragraph(paragraph)) = cell.blocks().next() else {
            panic!("no paragraph");
        };
        let size = formats.paragraph_mark(paragraph).size().0;
        assert_eq!(size, f64::from(10 + n % 40) / 2.0, "table {n}");
        let justification = if n % 2 == 0 {
            Justification::Center
        } else {
            Justification::Right
        };
        assert_eq!(
            formats.paragraph(paragraph).justification(),
            justification,
            "table {n}"
        );
        let (texts, roots, _, _) = formats.kept_table_texts();
        assert!(texts <= 256 && roots <= 256, "{texts} {roots}");
        n += 1;
    }
    assert_eq!(n, 300);
}

#[test]
fn works_out_a_long_style_chain_once_for_the_cells_of_every_table_style() {
    // From the review of #360: each table text kept its own copy of every paragraph
    // style chain, so 257 table styles in turn, each forgetting the oldest table text,
    // over cells in the last of 3,000 styles based on one another, rebuilt the chain for
    // every cell and kept up to 256 copies of it (2 GB). The chain is worked out once,
    // and each cell lays it over its table text in one step.
    let chain = 3000;
    let mut styles: String = (0..257)
        .map(|n| {
            format!(
                r#"<w:style w:type="table" w:styleId="S{n}"><w:name w:val="S{n}"/><w:rPr><w:sz w:val="{}"/></w:rPr><w:pPr><w:tabs><w:tab w:val="left" w:pos="{}"/></w:tabs></w:pPr></w:style>"#,
                10 + n % 40,
                100 + n
            )
        })
        .collect();
    for k in 0..chain {
        let based = if k == 0 {
            String::new()
        } else {
            format!(r#"<w:basedOn w:val="P{}"/>"#, k - 1)
        };
        // Each style sets one stop, and the last clears the table styles' stop at 100.
        let tabs = if k == chain - 1 {
            r#"<w:tab w:val="clear" w:pos="100"/>"#.to_owned()
        } else {
            format!(r#"<w:tab w:val="left" w:pos="{}"/>"#, 90_000 - 10 * k)
        };
        styles.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="P{k}"><w:name w:val="P{k}"/>{based}<w:pPr><w:tabs>{tabs}</w:tabs></w:pPr></w:style>"#
        ));
    }
    let tables = 600;
    let body: String = (0..tables)
        .map(|n| {
            format!(
                r#"<w:tbl><w:tblPr><w:tblStyle w:val="S{}"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p><w:pPr><w:pStyle w:val="P{}"/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
                n % 257,
                chain - 1
            )
        })
        .collect();
    let document = open_with(&styles, &body);
    let mut formats = document.formats();
    let mut n = 0;
    for block in document.sections().next().unwrap().blocks() {
        let Block::Table(table) = block else {
            continue;
        };
        let cell = table.rows().next().unwrap().cells().next().unwrap();
        let Some(Block::Paragraph(paragraph)) = cell.blocks().next() else {
            panic!("no paragraph");
        };
        let tabs = formats.paragraph(paragraph).tabs().to_vec();
        // The table style's stop at 100 + n % 257 stays, but at 100, which the chain
        // clears; the chain's stops are bounded to the lowest positions.
        let table_stop = 100 + u32::try_from(n % 257).unwrap();
        assert_eq!(
            tabs.iter()
                .any(|tab| tab.position().0 == f64::from(table_stop) / 20.0),
            table_stop != 100,
            "table {n}"
        );
        let (texts, roots, chains, cells) = formats.kept_table_texts();
        assert!(texts <= 256 && roots <= 256, "{texts} {roots}");
        assert!(chains <= chain && cells <= 4096, "{chains} {cells}");
        n += 1;
    }
    assert_eq!(n, tables);
    // Each style of the chain was read once, and each cell laid the chain over its table
    // text once.
    assert_eq!(formats.work().cell_style_reads, chain);
    assert_eq!(formats.work().cell_levels, tables);
}

#[test]
fn reads_a_paragraph_style_once_for_the_cells_of_every_table_style() {
    // The style chains of cells are laid over each table text, but what a paragraph style
    // writes itself is read once: 300 tables in styles of their own, each cell in one
    // paragraph style based on another, read two styles.
    let mut styles: String = (0..300)
        .map(|n| {
            format!(
                r#"<w:style w:type="table" w:styleId="S{n}"><w:name w:val="S{n}"/><w:rPr><w:sz w:val="{}"/></w:rPr></w:style>"#,
                10 + n % 40
            )
        })
        .collect();
    styles.push_str(r#"<w:style w:type="paragraph" w:styleId="Base"><w:name w:val="Base"/><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Cell"><w:name w:val="Cell"/><w:basedOn w:val="Base"/><w:pPr><w:jc w:val="center"/></w:pPr></w:style>"#);
    let body: String = (0..300)
        .map(|n| {
            format!(
                r#"<w:tbl><w:tblPr><w:tblStyle w:val="S{n}"/></w:tblPr><w:tblGrid/><w:tr><w:tc><w:p><w:pPr><w:pStyle w:val="Cell"/></w:pPr></w:p></w:tc></w:tr></w:tbl><w:p/>"#
            )
        })
        .collect();
    let document = open_with(&styles, &body);
    let mut formats = document.formats();
    for block in document.sections().next().unwrap().blocks() {
        let Block::Table(table) = block else {
            continue;
        };
        let cell = table.rows().next().unwrap().cells().next().unwrap();
        let Some(Block::Paragraph(paragraph)) = cell.blocks().next() else {
            panic!("no paragraph");
        };
        let format = formats.paragraph(paragraph);
        assert_eq!(format.justification(), Justification::Center);
        assert_eq!(format.tabs().len(), 1);
    }
    assert_eq!(formats.work().cell_style_reads, 2);
}
