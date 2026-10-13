//! Opens the input as a Word document and walks everything the model read from it:
//! sections, blocks, tables, inline content and runs, the text, and the XML of every
//! element kept without being understood, which must read back as XML.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_doc::{Block, Inline, Limits, LoadOptions, Opaque, RunContent};
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
    let mut blocks: Vec<Block<'_>> = document.sections().flat_map(|s| s.blocks()).collect();
    while let Some(block) = blocks.pop() {
        match block {
            Block::Paragraph(paragraph) => {
                assert!(document.paragraph(paragraph.id()).is_some());
                let _ = paragraph.properties();
                let _ = paragraph.text();
                let mut inlines: Vec<Inline<'_>> = paragraph.inlines().collect();
                while let Some(inline) = inlines.pop() {
                    match inline {
                        Inline::Run(run) => {
                            let _ = run.properties();
                            for item in run.content() {
                                if let RunContent::Other(opaque) = item {
                                    check(opaque);
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
                        Inline::Other(opaque) => check(opaque),
                        _ => {}
                    }
                }
            }
            Block::Table(table) => {
                assert!(document.table(table.id()).is_some());
                let _ = table.grid_columns().count();
                let _ = table.properties();
                for row in table.rows() {
                    let _ = (row.properties(), row.table_property_exceptions());
                    for cell in row.cells() {
                        let _ = cell.properties();
                        blocks.extend(cell.blocks());
                    }
                }
            }
            Block::ContentControl(control) => blocks.extend(control.blocks()),
            Block::Other(opaque) => check(opaque),
            _ => {}
        }
    }
});

/// An element kept as written must write back as well-formed XML.
fn check(opaque: Opaque<'_>) {
    let xml = opaque.to_xml();
    let input = Input::decode(xml.as_bytes()).unwrap_or_else(|err| panic!("{err}: {xml}"));
    let limits = officewright_opc::Limits::new().with_max_xml_depth(1 << 20);
    let mut reader = input.reader(&limits);
    while reader
        .read_event()
        .unwrap_or_else(|err| panic!("{err}: {xml}"))
        .is_some()
    {}
}
