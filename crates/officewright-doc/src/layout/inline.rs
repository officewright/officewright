//! The content of a paragraph as IR inlines: text, tabs, breaks, symbols and fields.
//!
//! Containers (hyperlinks, content controls, revisions, simple fields) are walked with a
//! stack, never recursively. Revisions are taken as accepted: insertions and content
//! moved here are kept, deletions and content moved away are left out.

use officewright_layout::ReportKind;
use officewright_layout::ir;

use super::{Once, Translator, is_marker};
use crate::model::{
    BreakClear, BreakKind, CharacterFormat, FieldCharKind, Inline, Paragraph, RevisionKind, Run,
    RunContent,
};

/// The longest part of a field's instruction read: far more than the name and switches
/// of any field the layout computes.
const MAX_INSTRUCTION: usize = 1024;

impl<'a> Translator<'a> {
    /// The inlines of a paragraph. `mark` is the style of its mark, which a simple field
    /// takes until its result gives it one.
    pub(super) fn inlines(
        &mut self,
        paragraph: Paragraph<'a>,
        mark: ir::RunStyleId,
    ) -> Vec<ir::Inline> {
        let mut content = Content::default();
        // Each open container, and for a simple field how many fields were open before
        // it: when its content ends, it closes, with any field begun inside it that did
        // not end there.
        let mut stack: Vec<(Box<dyn Iterator<Item = Inline<'a>> + 'a>, Option<usize>)> =
            vec![(Box::new(paragraph.inlines()), None)];
        while let Some((top, _)) = stack.last_mut() {
            let Some(inline) = top.next() else {
                if let Some((_, Some(open))) = stack.pop() {
                    while content.fields.len() > open {
                        content.end();
                    }
                }
                continue;
            };
            match inline {
                Inline::Run(run) => self.run(run, &mut content),
                Inline::Hyperlink(link) => stack.push((Box::new(link.inlines()), None)),
                Inline::ContentControl(control) => stack.push((Box::new(control.inlines()), None)),
                Inline::Revision(revision) => match revision.kind() {
                    RevisionKind::Insertion | RevisionKind::MoveTo => {
                        stack.push((Box::new(revision.inlines()), None));
                    }
                    RevisionKind::Deletion | RevisionKind::MoveFrom => {}
                },
                Inline::SimpleField(field) => {
                    let open = content.fields.len();
                    content.begin(mark);
                    content.instruction(field.instruction());
                    content.separate();
                    stack.push((Box::new(field.inlines()), Some(open)));
                }
                Inline::Other(other) => match other.local_name() {
                    "oMath" | "oMathPara" => {
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            "an equation is left out".into()
                        });
                    }
                    name if is_marker(name) => {}
                    name => {
                        let name = name.to_owned();
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            format!("an inline {name} element is left out")
                        });
                    }
                },
            }
        }
        content.finish()
    }

    /// The content of one run.
    fn run(&mut self, run: Run<'a>, content: &mut Content) {
        let format = self.formats.run(run);
        let style = self.run_style(&format);
        // Whether the run shows text, which its features would change, and whether it
        // holds a tab or a picture in the line, which a border around it would surround.
        let (mut shows, mut boxed) = (false, false);
        for item in run.content() {
            match item {
                RunContent::Text(text) => {
                    shows = true;
                    content.text(text, style);
                }
                // Tracked changes are taken as accepted, and comments are left out, as
                // Word's PDF of the document without markup shows them (the references are
                // exported with ExportAsFixedFormat's Item 0, wdExportDocumentContent), so
                // neither is listed (#303).
                RunContent::DeletedText(_) | RunContent::DeletedFieldCode(_) => {}
                RunContent::FieldCode(text) => content.instruction(text),
                RunContent::Tab => {
                    boxed = true;
                    content.push(ir::Inline::tab(style));
                }
                RunContent::Break(brk) => content.push(match brk.kind() {
                    BreakKind::Line => ir::Inline::line_break(clear(brk.clear()), style),
                    BreakKind::Page => ir::Inline::page_break(style),
                    BreakKind::Column => ir::Inline::column_break(style),
                }),
                RunContent::CarriageReturn => {
                    content.push(ir::Inline::line_break(ir::BreakClear::None, style));
                }
                RunContent::Symbol { font, char } => {
                    shows = true;
                    // w:sym writes four hexadecimal digits.
                    let Ok(code) = u16::try_from(u32::from(char)) else {
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            format!("a symbol beyond U+FFFF ({:X}) is left out", u32::from(char))
                        });
                        continue;
                    };
                    let font = self.font_name(font.unwrap_or(format.fonts().ascii()));
                    content.push(ir::Inline::symbol(font, code, style));
                }
                RunContent::FieldChar(FieldCharKind::Begin) => content.begin(style),
                RunContent::FieldChar(FieldCharKind::Separate) => content.separate(),
                RunContent::FieldChar(FieldCharKind::End) => content.end(),
                RunContent::NoBreakHyphen => {
                    shows = true;
                    content.push(ir::Inline::no_break_hyphen(style));
                }
                RunContent::SoftHyphen => content.push(ir::Inline::soft_hyphen(style)),
                RunContent::FootnoteReference(id) => {
                    if let Some(note) = self.note_reference(false, id) {
                        shows = true;
                        content.push(ir::Inline::note_reference(note, false, style));
                    }
                }
                RunContent::EndnoteReference(id) => {
                    if let Some(note) = self.note_reference(true, id) {
                        shows = true;
                        content.push(ir::Inline::note_reference(note, false, style));
                    }
                }
                RunContent::NoteNumber => {
                    shows = true;
                    content.push(ir::Inline::note_number(style));
                }
                RunContent::Separator => content.push(ir::Inline::separator(style)),
                RunContent::ContinuationSeparator => {
                    content.push(ir::Inline::continuation_separator(style));
                }
                RunContent::Drawing(drawing) => {
                    if let Some(object) = self.drawing(drawing) {
                        boxed |= drawing.is_inline();
                        content.push(object);
                    }
                }
                RunContent::CommentReference(_) | RunContent::LastRenderedPageBreak => {}
                // The legacy page number: a PAGE field (ECMA-376 Part 1, §17.3.3.22).
                RunContent::Other(other) if other.local_name() == "pgNum" => {
                    if content.fields.is_empty() {
                        shows = true;
                        content.push(ir::Inline::Field(ir::Field::new(
                            ir::FieldKind::Page,
                            style,
                        )));
                    } else {
                        // The IR holds no field inside another's result, and the page
                        // number has no result to stand for it.
                        self.leave_out(ReportKind::FeatureUnsupported, || {
                            "a page number (w:pgNum) inside a field is left out".into()
                        });
                    }
                }
                RunContent::Other(other) => {
                    let message = match other.local_name() {
                        "pict" => "a VML drawing is left out",
                        "object" => "an embedded object is left out",
                        "ptab" => "an absolute position tab is left out",
                        name if is_marker(name) => continue,
                        _ => "an element in a run is left out",
                    };
                    let name = other.local_name().to_owned();
                    self.leave_out(ReportKind::FeatureUnsupported, || {
                        format!("{message} (w:{name})")
                    });
                }
            }
        }
        if !format.hidden() && (shows || boxed) {
            self.report_run_features(&format, shows);
        }
    }

    /// Lists the features of a run's formatting that the IR's run style has no place
    /// for, so that the PDF shows the run without them (#303). Only a run that is not
    /// hidden counts, and one that `shows` text for the features of its glyphs: a hidden
    /// run, or one that holds only a field's instruction, shows nothing they could
    /// change. A border surrounds a tab or a picture in the line too.
    fn report_run_features(&mut self, format: &CharacterFormat, shows: bool) {
        if format.border {
            self.report_once(
                Once::RunBorder,
                "borders around text (w:bdr) are not drawn, here and wherever else the document has them",
            );
        }
        if !shows {
            return;
        }
        for (on, once, message) in [
            (
                format.emboss(),
                Once::Emboss,
                "embossed text (w:emboss) is drawn plain, here and wherever else the document has it",
            ),
            (
                format.imprint(),
                Once::Imprint,
                "engraved text (w:imprint) is drawn plain, here and wherever else the document has it",
            ),
            (
                format.outline(),
                Once::Outline,
                "outlined text (w:outline) is drawn plain, here and wherever else the document has it",
            ),
            (
                format.shadow(),
                Once::Shadow,
                "shadowed text (w:shadow) is drawn without its shadow, here and wherever else the document has it",
            ),
            (
                format.emphasis_mark,
                Once::EmphasisMark,
                "emphasis marks (w:em) are not drawn, here and wherever else the document has them",
            ),
        ] {
            if on {
                self.report_once(once, message);
            }
        }
    }
}

fn clear(clear: BreakClear) -> ir::BreakClear {
    match clear {
        BreakClear::None => ir::BreakClear::None,
        BreakClear::Left => ir::BreakClear::Left,
        BreakClear::Right => ir::BreakClear::Right,
        BreakClear::All => ir::BreakClear::All,
    }
}

/// The inlines of a paragraph as they are read, with the complex fields open around
/// them. Fields nest; the IR holds no field in the result of another, so the result of
/// an inner field becomes part of the outer one's. The results of the open fields share
/// one list, each from where it starts, so an inner field's result is never moved: every
/// inline is added once, however deep the fields nest.
#[derive(Default)]
struct Content {
    out: Vec<ir::Inline>,
    results: Vec<ir::Inline>,
    fields: Vec<Frame>,
    /// The places in `fields` of the fields between their beginning and their separator.
    instructions: Vec<usize>,
    /// The places in `fields` of the results with no inline that has a style yet.
    unstyled: Vec<usize>,
}

/// A field being read.
enum Frame {
    /// Between its beginning and its separator: its instruction, and the style it began
    /// in.
    Instruction { text: String, style: ir::RunStyleId },
    /// Its result, from `start` in [`Content::results`], and the style of a value the
    /// layout computes: that of the result's first inline that has one, else the one it
    /// began in.
    Result {
        kind: ir::FieldKind,
        format: Option<ir::NumberFormat>,
        start: usize,
        style: ir::RunStyleId,
    },
}

impl Content {
    fn push(&mut self, inline: ir::Inline) {
        #[cfg(test)]
        super::work(|work| work.inlines += 1);
        match self.fields.last() {
            None => self.out.push(inline),
            // Content between a field's beginning and its separator is its instruction,
            // which shows nothing.
            Some(Frame::Instruction { .. }) => {}
            Some(Frame::Result { .. }) => {
                // The inline is the first with a style of every result it reaches that
                // has none yet: those above the innermost instruction, whose content is
                // not shown. Each result gets its style once.
                if let Some(first) = style_of(&inline) {
                    let above = self.instructions.last().map_or(0, |&at| at + 1);
                    while let Some(&at) = self.unstyled.last() {
                        if at < above {
                            break;
                        }
                        if let Some(Frame::Result { style, .. }) = self.fields.get_mut(at) {
                            *style = first;
                        }
                        self.unstyled.pop();
                    }
                }
                self.results.push(inline);
            }
        }
    }

    /// Text, with tabs as inlines of their own and other control characters, which IR
    /// text does not hold, as spaces (line ends) or left out.
    fn text(&mut self, text: &str, style: ir::RunStyleId) {
        let mut piece = String::new();
        for c in text.chars() {
            match c {
                '\t' => {
                    if !piece.is_empty() {
                        self.push(ir::Inline::text(std::mem::take(&mut piece), style));
                    }
                    self.push(ir::Inline::tab(style));
                }
                '\n' | '\r' => piece.push(' '),
                c if c.is_control() => {}
                c => piece.push(c),
            }
        }
        if !piece.is_empty() {
            self.push(ir::Inline::text(piece, style));
        }
    }

    fn begin(&mut self, style: ir::RunStyleId) {
        self.instructions.push(self.fields.len());
        self.fields.push(Frame::Instruction {
            text: String::new(),
            style,
        });
    }

    fn instruction(&mut self, more: &str) {
        if let Some(Frame::Instruction { text, .. }) = self.fields.last_mut() {
            for c in more.chars() {
                if text.len() + c.len_utf8() > MAX_INSTRUCTION {
                    break;
                }
                text.push(c);
            }
        }
    }

    fn separate(&mut self) {
        let Some(Frame::Instruction { text, style }) = self.fields.last() else {
            // A separator outside an instruction is stray; Word ignores it.
            return;
        };
        let (kind, format) = parse(text);
        let style = *style;
        let start = self.results.len();
        if let Some(top) = self.fields.last_mut() {
            *top = Frame::Result {
                kind,
                format,
                start,
                style,
            };
        }
        self.instructions.pop();
        self.unstyled.push(self.fields.len().saturating_sub(1));
    }

    fn end(&mut self) {
        let at = self.fields.len().saturating_sub(1);
        let (kind, format, start, style) = match self.fields.pop() {
            // An end without a beginning is stray.
            None => return,
            Some(Frame::Instruction { text, style }) => {
                self.instructions.pop();
                let (kind, format) = parse(&text);
                (kind, format, None, style)
            }
            Some(Frame::Result {
                kind,
                format,
                start,
                style,
            }) => {
                if self.unstyled.last() == Some(&at) {
                    self.unstyled.pop();
                }
                (kind, format, Some(start), style)
            }
        };
        match self.fields.last() {
            // The outermost field: its result is all the shared list holds from its start.
            None => {
                let result: Vec<ir::Inline> = match start {
                    Some(start) => self.results.drain(start..).collect(),
                    None => Vec::new(),
                };
                self.out.push(ir::Inline::Field(
                    ir::Field::new(kind, style)
                        .with_result(result)
                        .with_format(format),
                ));
            }
            // Inside another field's result, the result stays where it is, part of it.
            Some(Frame::Result { .. }) => {}
            // Inside an instruction, it is not shown.
            Some(Frame::Instruction { .. }) => {
                if let Some(start) = start {
                    self.results.truncate(start);
                }
            }
        }
    }

    /// The inlines, with every field still open closed: a field that ends in a later
    /// paragraph keeps the result it has here, and the rest of that result reads as plain
    /// content there.
    fn finish(mut self) -> Vec<ir::Inline> {
        while !self.fields.is_empty() {
            self.end();
        }
        self.out
    }
}

/// The style of an inline that has one.
fn style_of(inline: &ir::Inline) -> Option<ir::RunStyleId> {
    match inline {
        ir::Inline::Text { style, .. }
        | ir::Inline::Tab { style, .. }
        | ir::Inline::Break { style, .. }
        | ir::Inline::Symbol { style, .. }
        | ir::Inline::SoftHyphen { style, .. }
        | ir::Inline::NoBreakHyphen { style, .. } => Some(*style),
        _ => None,
    }
}

/// What a field is and the number format its `\*` switch gives, from its instruction:
/// the name first, case-insensitive (`PAGE`, `NUMPAGES`, `SECTIONPAGES` are computed by
/// the layout), then switches. Of the format switches, the case of `ROMAN` and
/// `ALPHABETIC` gives the case of the result; MERGEFORMAT, CHARFORMAT and the case
/// switches are not number formats.
fn parse(instruction: &str) -> (ir::FieldKind, Option<ir::NumberFormat>) {
    let mut words = instruction.split_whitespace();
    let name = words.next().unwrap_or_default();
    let kind = match name.to_ascii_uppercase().as_str() {
        "PAGE" => ir::FieldKind::Page,
        "NUMPAGES" => ir::FieldKind::NumPages,
        "SECTIONPAGES" => ir::FieldKind::SectionPages,
        _ => ir::FieldKind::Other(name.to_owned()),
    };
    let mut format = None;
    while let Some(word) = words.next() {
        if word != "\\*" {
            continue;
        }
        let Some(argument) = words.next() else {
            break;
        };
        let upper = argument.chars().next().is_some_and(char::is_uppercase);
        let found = match argument.to_ascii_lowercase().as_str() {
            "mergeformat" | "charformat" | "upper" | "lower" | "firstcap" | "caps" => continue,
            "arabic" => ir::NumberFormat::Decimal,
            "roman" if upper => ir::NumberFormat::UpperRoman,
            "roman" => ir::NumberFormat::LowerRoman,
            "alphabetic" if upper => ir::NumberFormat::UpperLetter,
            "alphabetic" => ir::NumberFormat::LowerLetter,
            "ordinal" => ir::NumberFormat::Ordinal,
            _ => ir::NumberFormat::Other(argument.to_owned()),
        };
        format = Some(found);
        break;
    }
    (kind, format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_name_and_the_number_format_of_a_field() {
        let cases = [
            ("PAGE", ir::FieldKind::Page, None),
            (" page  \\* MERGEFORMAT ", ir::FieldKind::Page, None),
            (
                "NUMPAGES \\* roman",
                ir::FieldKind::NumPages,
                Some(ir::NumberFormat::LowerRoman),
            ),
            (
                "SECTIONPAGES \\* ROMAN",
                ir::FieldKind::SectionPages,
                Some(ir::NumberFormat::UpperRoman),
            ),
            (
                "PAGE \\* Arabic \\* MERGEFORMAT",
                ir::FieldKind::Page,
                Some(ir::NumberFormat::Decimal),
            ),
            (
                "PAGE \\* MERGEFORMAT \\* alphabetic",
                ir::FieldKind::Page,
                Some(ir::NumberFormat::LowerLetter),
            ),
            (
                "PAGE \\* ALPHABETIC",
                ir::FieldKind::Page,
                Some(ir::NumberFormat::UpperLetter),
            ),
            (
                "PAGE \\* CardText",
                ir::FieldKind::Page,
                Some(ir::NumberFormat::Other("CardText".into())),
            ),
            (
                "DATE \\@ \"d MMMM yyyy\"",
                ir::FieldKind::Other("DATE".into()),
                None,
            ),
            ("PAGE \\*", ir::FieldKind::Page, None),
            ("", ir::FieldKind::Other(String::new()), None),
        ];
        for (instruction, kind, format) in cases {
            assert_eq!(parse(instruction), (kind, format), "{instruction}");
        }
    }
}
