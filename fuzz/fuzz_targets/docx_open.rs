//! Opens the input as a Word document and walks everything the model read from it:
//! sections, headers, footers and notes, blocks, tables, inline content and runs, the
//! text, the programs of embedded fonts, and the XML of the drawings and of the elements
//! kept without being understood, which must read back as XML (the first 16 MiB of it,
//! `WRITE_BACK_BYTES`). The formatting in
//! effect of every paragraph, run, paragraph mark, table, row and cell is resolved, and
//! must not depend on what the resolver remembered from earlier ones. The document's
//! translation into the layout IR must be valid IR.
#![no_main]

use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
use officewright_doc::{Block, HeaderFooterKind, Inline, Limits, LoadOptions, RunContent};
use officewright_opc::xml::Input;

fuzz_target!(|data: &[u8]| {
    // Small enough that a bomb shows up as a limit, not as an out-of-memory crash.
    let limits = Limits::new()
        .with_max_part_size(1 << 20)
        .with_max_total_size(4 << 20)
        .with_max_nodes(1 << 18);
    let Ok(document) = LoadOptions::new().with_limits(limits).open_bytes(data) else {
        return;
    };
    let _ = document.text();
    let _ = document.diagnostics();
    if let Some(theme) = document.theme() {
        let _ = (theme.colors().map(|colors| colors.name()), theme.fonts());
    }
    for font in document.fonts() {
        let _ = (font.panose(), font.signature(), font.embedded().len());
        for embedded in font.embedded() {
            let _ = embedded.program().map(<[u8]>::len);
        }
    }
    let settings = document.settings();
    let _ = (
        settings.effective_compatibility_mode(),
        settings.compatibility_options().count(),
    );
    let numbering = document.numbering();
    for definition in numbering.definitions() {
        let _ = (
            definition.identifier(),
            definition.multi_level_type(),
            definition.name(),
        );
        let _ = (definition.levels(), definition.level(0));
    }
    for instance in numbering.instances() {
        let _ = (instance.definition(), instance.overrides());
    }
    let styles = document.styles();
    let _ = styles.default_run_properties();
    let _ = styles.default_paragraph_properties();
    for style in styles.iter() {
        let _ = (style.name(), style.based_on(), style.run_properties());
        let _ = style.paragraph_properties();
        let _ = (
            style.table_properties(),
            style.row_properties(),
            style.cell_properties(),
        );
        assert!(styles.get(style.id()).is_some());
    }
    let mut formats = document.formats();
    // A fresh resolver walks every style chain again, so only the first paragraphs and
    // runs are compared, each with a budget of its own: the checks stay, the cost does
    // not grow with their number × chain length.
    let (mut paragraphs_compared, mut runs_compared, mut cells_compared) = (0, 0, 0);
    let mut write_back = WriteBack::default();
    let mut blocks: Vec<Block<'_>> = document.sections().flat_map(|s| s.blocks()).collect();
    // The headers, footers and notes, each walked like the body. Each header and footer
    // is walked once, however many sections have it: thousands of sections may name or
    // inherit a few.
    let mut walked = HashSet::new();
    for section in document.sections() {
        for kind in [
            HeaderFooterKind::Default,
            HeaderFooterKind::First,
            HeaderFooterKind::Even,
        ] {
            for story in [section.header(kind), section.footer(kind)]
                .into_iter()
                .flatten()
            {
                if walked.insert(story) {
                    blocks.extend(story.blocks());
                }
            }
        }
    }
    for note in document.notes() {
        let _ = (note.id(), note.note_type(), note.is_endnote());
        blocks.extend(note.blocks());
    }
    while let Some(block) = blocks.pop() {
        match block {
            Block::Paragraph(paragraph) => {
                assert!(document.paragraph(paragraph.id()).is_some());
                let _ = paragraph.properties();
                let _ = paragraph.text();
                let _ = formats.paragraph_mark(paragraph);
                let _ = formats.list_label(paragraph);
                let format = formats.paragraph(paragraph);
                if paragraphs_compared < 256 {
                    paragraphs_compared += 1;
                    assert_eq!(format, paragraph.format());
                }
                for run in paragraph.runs() {
                    let format = formats.run(run);
                    if runs_compared < 256 {
                        runs_compared += 1;
                        assert_eq!(format, run.format());
                    }
                }
                let mut inlines: Vec<Inline<'_>> = paragraph.inlines().collect();
                while let Some(inline) = inlines.pop() {
                    match inline {
                        Inline::Run(run) => {
                            let _ = run.properties();
                            for item in run.content() {
                                match item {
                                    RunContent::Other(opaque) => {
                                        write_back.check(|| opaque.to_xml())
                                    }
                                    RunContent::Drawing(drawing) => {
                                        write_back.check(|| drawing.to_xml());
                                        let _ = (drawing.is_inline(), drawing.width());
                                        let _ = (drawing.height(), drawing.effect_extent());
                                        let _ = drawing.anchor();
                                        if let Some(picture) = drawing.picture() {
                                            let _ = (picture.crop(), picture.rotation());
                                            let _ = (
                                                picture.flip_horizontal(),
                                                picture.flip_vertical(),
                                            );
                                            let _ =
                                                picture.image().map(|image| image.bytes().len());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Inline::Hyperlink(link) => {
                            let _ = (link.target(), link.anchor());
                            inlines.extend(link.inlines());
                        }
                        Inline::ContentControl(control) => inlines.extend(control.inlines()),
                        Inline::Revision(revision) => {
                            let _ = (revision.kind(), revision.author(), revision.date());
                            inlines.extend(revision.inlines());
                        }
                        Inline::SimpleField(field) => {
                            let _ = field.instruction();
                            inlines.extend(field.inlines());
                        }
                        Inline::Other(opaque) => write_back.check(|| opaque.to_xml()),
                        _ => {}
                    }
                }
            }
            Block::Table(table) => {
                assert!(document.table(table.id()).is_some());
                let _ = table.grid_columns().count();
                let _ = table.properties();
                let properties = formats.table_properties(table);
                let _ = formats.table_look(table);
                for row in table.rows() {
                    let _ = (row.properties(), row.table_property_exceptions());
                    let row_format = formats.row_properties(row);
                    for cell in row.cells() {
                        let _ = cell.properties();
                        let cell_format = formats.cell_properties(cell);
                        let _ = formats.cell_conditions(cell);
                        // A fresh resolver works the table out again: the first cells
                        // only, as for paragraphs and runs.
                        if cells_compared < 64 {
                            cells_compared += 1;
                            let mut fresh = document.formats();
                            assert_eq!(cell_format, fresh.cell_properties(cell));
                            assert_eq!(row_format, fresh.row_properties(row));
                            assert_eq!(properties, fresh.table_properties(table));
                        }
                        blocks.extend(cell.blocks());
                    }
                }
            }
            Block::ContentControl(control) => blocks.extend(control.blocks()),
            Block::Other(opaque) => write_back.check(|| opaque.to_xml()),
            _ => {}
        }
    }
    // The translation into the layout IR gives IR the layout accepts.
    let translation = document.to_layout_ir();
    if let Err(err) = translation.document().validate() {
        panic!("{err}");
    }
});

/// The kept elements and drawings written back and read again. Each one's XML declares
/// every namespace it needs, so a small file of many elements that each need a long
/// namespace bound outside them writes back gigabytes (#283): the check stops after
/// [`WRITE_BACK_BYTES`] in all.
#[derive(Default)]
struct WriteBack {
    written: usize,
}

/// How many bytes of XML the target writes back and reads again at most.
const WRITE_BACK_BYTES: usize = 16 << 20;

impl WriteBack {
    fn check(&mut self, xml: impl FnOnce() -> String) {
        if self.written >= WRITE_BACK_BYTES {
            return;
        }
        let xml = xml();
        self.written = self.written.saturating_add(xml.len());
        check(&xml);
    }
}

/// An element kept as written, or a drawing, must write back as well-formed XML.
fn check(xml: &str) {
    let input = Input::decode(xml.as_bytes()).unwrap_or_else(|err| panic!("{err}: {xml}"));
    // Well-formedness is checked, not the limits the source met: the declarations the
    // output adds can take an element past the attributes its source had room for.
    let limits = officewright_opc::Limits::new()
        .with_max_xml_depth(1 << 20)
        .with_max_xml_attributes(usize::MAX);
    let mut reader = input.reader(&limits);
    while reader
        .read_event()
        .unwrap_or_else(|err| panic!("{err}: {xml}"))
        .is_some()
    {}
}
