//! Prints the layout IR a Word document translates into, as text, so that it can be
//! compared with the document by eye.
//!
//! ```text
//! cargo run -p officewright-doc --features pdf --example ir_dump -- FILE.docx
//! ```
//!
//! It prints the settings; each section's page and columns, its headers and footers; each
//! paragraph's formatting, list item and mark; and its content, each piece with its run
//! style, and each note reference with its note's text; each table's properties, borders
//! and rows, and each cell's span, alignment, shading, and its own margins and borders.
//! Then it lists what the translation left out, and whether the IR passes `validate`.

use std::process::ExitCode;

use officewright_doc::Document;
use officewright_layout::ir;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [file] = args.as_slice() else {
        eprintln!("usage: ir_dump FILE");
        return ExitCode::FAILURE;
    };
    let document = match Document::open(file) {
        Ok(document) => document,
        Err(err) => {
            eprintln!("{file}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let translation = document.to_layout_ir();
    let ir = translation.document();
    let settings = ir.settings();
    println!(
        "settings: {:?}, default tab {} pt, decimal {:?}, flags {:?}",
        settings.compat_mode(),
        settings.default_tab_stop().0,
        settings.decimal_separator(),
        settings
            .compat_flags()
            .iter()
            .map(ir::CompatFlag::name)
            .collect::<Vec<_>>()
    );
    for (n, section) in ir.sections().iter().enumerate() {
        let page = section.page();
        let margins = page.margins();
        println!(
            "section {n}: page {} x {} pt {:?}, margins {}/{}/{}/{} pt, {} columns, start {:?}",
            page.size().width().0,
            page.size().height().0,
            page.orientation(),
            margins.top().0,
            margins.right().0,
            margins.bottom().0,
            margins.left().0,
            section.columns().count(),
            section.start()
        );
        for (name, set) in [("header", section.headers()), ("footer", section.footers())] {
            for (kind, blocks) in [
                ("primary", set.primary()),
                ("first", set.first()),
                ("even", set.even()),
            ] {
                if !blocks.is_empty() {
                    println!("  {name} {kind}");
                    print_blocks(ir, blocks, "    ");
                }
            }
        }
        print_blocks(ir, section.blocks(), "  ");
    }
    for entry in translation.left_out() {
        println!("left out: {entry}");
    }
    match ir.validate() {
        Ok(()) => println!("valid"),
        Err(err) => {
            println!("not valid: {err}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn print_blocks(ir: &ir::Document, blocks: &[ir::Block], indent: &str) {
    for block in blocks {
        match block {
            ir::Block::Paragraph(paragraph) => print_paragraph(ir, paragraph, indent),
            ir::Block::Table(table) => {
                let props = table.props();
                println!(
                    "{indent}table {:?} {:?}, indent {} pt, grid {:?}",
                    props.layout(),
                    props.width(),
                    props.indent().0,
                    table.grid().iter().map(|width| width.0).collect::<Vec<_>>()
                );
                println!(
                    "{indent}  {:?}, spacing {} pt, {:?}, shading {:?}",
                    props.alignment(),
                    props.cell_spacing().0,
                    props.cell_margins(),
                    props.shading()
                );
                println!("{indent}  {:?}", props.borders());
                for (n, row) in table.rows().iter().enumerate() {
                    println!(
                        "{indent}  row {n} height {:?}, header {}, cant split {}",
                        row.height(),
                        row.header(),
                        row.cant_split()
                    );
                    for cell in row.cells() {
                        println!(
                            "{indent}    cell span {} {:?} {:?}, {:?}, shading {:?}",
                            cell.grid_span(),
                            cell.width(),
                            cell.vertical_merge(),
                            cell.vertical_align(),
                            cell.shading()
                        );
                        if let Some(margins) = cell.margins() {
                            println!("{indent}      own {margins:?}");
                        }
                        if cell.borders() != ir::CellBorders::default() {
                            println!("{indent}      {:?}", cell.borders());
                        }
                        print_blocks(ir, cell.blocks(), &format!("{indent}      "));
                    }
                }
            }
            other => println!("{indent}{other:?}"),
        }
    }
}

fn print_paragraph(ir: &ir::Document, paragraph: &ir::Paragraph, margin: &str) {
    let props = paragraph.props();
    let indent = props.indent();
    let spacing = props.spacing();
    let mut line = format!(
        "{margin}paragraph {:?}, indent {}/{}/{} pt, spacing {}/{} pt, line {:?}, style key {}",
        props.alignment(),
        indent.start().0,
        indent.end().0,
        indent.first_line().0,
        spacing.before().0,
        spacing.after().0,
        spacing.line(),
        props.style_key().value()
    );
    if !props.tabs().is_empty() {
        let tabs: Vec<String> = props
            .tabs()
            .iter()
            .map(|tab| format!("{:?} {}", tab.kind(), tab.position().0))
            .collect();
        line.push_str(&format!(", tabs [{}]", tabs.join(", ")));
    }
    if let Some(item) = paragraph.list() {
        line.push_str(&format!(
            ", list level {} label {}",
            item.level(),
            style(ir, item.label_style())
        ));
    }
    println!("{line}");
    println!("{margin}  mark {}", style(ir, paragraph.mark()));
    for inline in paragraph.inlines() {
        println!("{margin}  {}", describe(ir, inline));
    }
}

fn describe(ir: &ir::Document, inline: &ir::Inline) -> String {
    match inline {
        ir::Inline::Text {
            text, style: id, ..
        } => format!("text {text:?} {}", style(ir, *id)),
        ir::Inline::Tab { .. } => "tab".to_owned(),
        ir::Inline::Break { kind, .. } => format!("break {kind:?}"),
        ir::Inline::Symbol { font, code, .. } => format!("symbol {font} {code:04X}"),
        ir::Inline::SoftHyphen { .. } => "soft hyphen".to_owned(),
        ir::Inline::NoBreakHyphen { .. } => "no-break hyphen".to_owned(),
        ir::Inline::NoteReference { note, .. } => match ir.note(*note) {
            Some(note) => format!("{:?} reference [{}]", note.kind(), note_text(note)),
            None => "note reference".to_owned(),
        },
        ir::Inline::Field(field) => {
            let result: Vec<String> = field
                .result()
                .iter()
                .map(|inline| describe(ir, inline))
                .collect();
            format!(
                "field {:?} format {:?} [{}]",
                field.kind(),
                field.format(),
                result.join("; ")
            )
        }
        ir::Inline::Object(object) => {
            format!(
                "picture in the line {}",
                object_text(ir, object.content(), object.size())
            )
        }
        ir::Inline::Anchored(object) => format!(
            "floating picture {}, across {:?} {:?}, down {:?} {:?}, wrap {:?}",
            object_text(ir, object.content(), object.size()),
            object.horizontal().relative_to(),
            object.horizontal().placement(),
            object.vertical().relative_to(),
            object.vertical().placement(),
            object.wrap()
        ),
        other => format!("{other:?}"),
    }
}

/// What an object shows and its size, briefly.
fn object_text(ir: &ir::Document, content: &ir::ObjectContent, size: ir::Size) -> String {
    let image = match content {
        ir::ObjectContent::Picture(picture) => ir.image(picture.image()),
        _ => None,
    };
    let image = image.map_or_else(
        || "of no image".to_owned(),
        |image| format!("{:?} of {} bytes", image.format(), image.bytes().len()),
    );
    format!("{image}, {:.1}x{:.1}pt", size.width().0, size.height().0)
}

/// A run style, briefly.
fn style(ir: &ir::Document, id: ir::RunStyleId) -> String {
    let Some(style) = ir.run_style(id) else {
        return "(no style)".to_owned();
    };
    let mut out = format!("{} {}pt", style.fonts().ascii(), style.size().0);
    for (on, name) in [
        (style.bold(), "bold"),
        (style.italic(), "italic"),
        (style.underline().is_some(), "underline"),
        (style.caps(), "caps"),
        (style.small_caps(), "small-caps"),
        (style.strike(), "strike"),
        (style.hidden(), "hidden"),
    ] {
        if on {
            out.push(' ');
            out.push_str(name);
        }
    }
    if let ir::ColorValue::Rgb(color) = style.color() {
        out.push_str(&format!(" {color:?}"));
    }
    out
}

/// The text of a note, briefly.
fn note_text(note: &ir::Note) -> String {
    note.blocks()
        .iter()
        .filter_map(|block| match block {
            ir::Block::Paragraph(paragraph) => Some(
                paragraph
                    .inlines()
                    .iter()
                    .filter_map(|inline| match inline {
                        ir::Inline::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" / ")
}
