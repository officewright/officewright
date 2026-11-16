//! What a document holds that the first release does not lay out (plan 1.3, the parts of
//! phase 2 it leaves out, and what phase 1 lays out otherwise and reports), found in its
//! markup. Gate row 11 asks the layout's report of every rendered document to name each
//! kind it holds, so that a user knows why our PDF differs from Word's.
//!
//! Only the stories Word shows are read: the body, the headers and footers in effect for
//! each section (a section's own of each kind and type, else the previous section's;
//! first-page ones only for a section with a title page of its own, even-page ones with
//! `w:evenAndOddHeaders`), the footnotes and the endnotes; not the styles, so content a style
//! gives is not found. Deleted text, the formatting a revision replaced, the fallbacks of
//! Markup Compatibility, and what groups of shapes, canvases and objects shown by a drawing
//! hold, which the translation leaves out whole, are skipped. Transitional and Strict names
//! are read alike.

use std::collections::{BTreeMap, BTreeSet};

use officewright_opc::xml::{Event, Input, Name, NsId, Reader, StartTag};
use officewright_opc::{Limits, Package, Part, PartName};

/// The kinds of content, each with the words of which an entry of the layout's report
/// about it names one, in lower case: words of the messages the layout and the translation
/// write for it, specific enough not to be found in a message about something else.
pub const KINDS: [(&str, &[&str]); 21] = [
    (
        "chart",
        &["a chart is left out", "a drawing that shows no picture"],
    ),
    ("SmartArt", &["a drawing that shows no picture"]),
    (
        "shape",
        &[
            "a drawing that shows no picture",
            "a vml drawing",
            "a group of vml shapes",
        ],
    ),
    ("embedded object", &["an embedded object"]),
    ("equation", &["an equation is left out"]),
    // Formats other than PNG and JPEG are drawn as same-size placeholders (phase 1).
    ("picture format", &["format are drawn as grey boxes"]),
    ("columns", &["text in columns"]),
    (
        "document grid",
        &["the document grid", "on a document grid"],
    ),
    ("line numbers", &["line numbers (w:lnnumtype)"]),
    ("page borders", &["page borders (w:pgborders)"]),
    // Phase 1 lays out text frames as ordinary paragraphs and floating tables in the text
    // flow, and reports both.
    (
        "text frame",
        &["a text frame (w:framepr)", "a drop cap (w:framepr)"],
    ),
    (
        "floating table",
        &["floating tables are laid out in the text"],
    ),
    ("vertical alignment", &["whatever its vertical alignment"]),
    // An entry may say instead that the object is not drawn at all.
    (
        "text wrapping",
        &[
            "text does not wrap around floating objects",
            "floating objects in table cells are not drawn",
            "floating objects in text boxes are not drawn",
        ],
    ),
    (
        "vertical text",
        &[
            "vertical text in table cells",
            "vertical text (w:textdirection)",
            "vertical text in a text box",
        ],
    ),
    ("rotated text box", &["a turned text box"]),
    ("linked text box", &["a chain of linked text boxes"]),
    // Tables and floating objects in a text box: basic text boxes hold paragraphs only.
    (
        "text box content",
        &["tables in text boxes", "floating objects in text boxes"],
    ),
    (
        "right-to-left text",
        &[
            "right-to-left text is laid out",
            "a right-to-left paragraph",
            "right-to-left tables",
        ],
    ),
    ("ruby", &["ruby"]),
    ("text fitting", &["text fitting"]),
];

/// The words of which an entry about content of `kind` names one; `None` for a kind this
/// runner does not know, from a report of another version.
pub fn words(kind: &str) -> Option<&'static [&'static str]> {
    KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, words)| *words)
}

/// Whether an entry's message names content of the kind with these words.
pub fn names(message: &str, words: &[&str]) -> bool {
    let message = message.to_lowercase();
    words.iter().any(|word| message.contains(word))
}

/// What the scan of a document finds.
#[derive(Debug, PartialEq)]
pub struct Scanned {
    /// The kinds of content, in the order of [`KINDS`].
    pub kinds: Vec<String>,
    /// The header and footer stories of Word's formatting export (`first-page-header`,
    /// `even-pages-footer`, ...) of a kind and type no section shows. A story of a type that
    /// some section shows is not listed even where a page shows some or none of its headers:
    /// a later section's own first-page header without a title page of its own, the default
    /// header of a one-page section with a title page, the even-page header of a one-page
    /// document.
    pub stories_not_shown: Vec<String>,
}

/// What the Word document at `path` holds.
pub fn scan(path: &std::path::Path) -> Result<Scanned, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    scan_bytes(bytes)
}

/// The content types of the main document part, without parameters.
const MAIN: [&str; 4] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    "application/vnd.ms-word.document.macroEnabled.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
    "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
];

/// The content types of the notes parts.
const NOTES: [&str; 2] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
];

/// The content type of a part, without parameters.
fn content_type<'p>(part: &Part<'p>) -> Option<&'p str> {
    part.content_type()
        .and_then(|kind| kind.split(';').next())
        .map(str::trim)
}

/// The stories of Word's formatting export of each kind and type of header and footer.
const STORY_NAMES: [(&str, &str, &str); 6] = [
    ("header", "default", "primary-header"),
    ("header", "first", "first-page-header"),
    ("header", "even", "even-pages-header"),
    ("footer", "default", "primary-footer"),
    ("footer", "first", "first-page-footer"),
    ("footer", "even", "even-pages-footer"),
];

/// What a package's stories hold.
pub fn scan_bytes(bytes: Vec<u8>) -> Result<Scanned, String> {
    let limits = Limits::new();
    let package = Package::from_bytes(bytes, &limits).map_err(|error| error.to_string())?;
    let main = package
        .parts()
        .find(|part| content_type(part).is_some_and(|kind| MAIN.contains(&kind)))
        .ok_or("the package has no main document part")?;
    let mut found = BTreeSet::new();
    let mut sections = Sections::default();
    scan_part(&main, &limits, &mut found, Some(&mut sections))
        .map_err(|error| format!("{}: {error}", main.name()))?;
    let even = even_and_odd_headers(&main, &limits)?;
    // Each section shows the headers and footers in effect for it: its own of each kind
    // and type, else the previous section's; a first-page one only with a title page of
    // its own, even-page ones only with w:evenAndOddHeaders.
    let mut current: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut shown_types: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut shown: Vec<PartName> = Vec::new();
    for section in &sections.0 {
        for (kind, which, id) in &section.references {
            current.insert((kind.as_str(), which.as_str()), id.as_str());
        }
        for (&(kind, which), id) in &current {
            let shows = match which {
                "first" => section.title_page,
                "even" => even,
                _ => true,
            };
            if !shows {
                continue;
            }
            shown_types.insert((kind, which));
            let target = main
                .relationships()
                .get(id)
                .and_then(|rel| rel.target_part());
            if let Some(target) = target {
                if !shown.contains(&target) {
                    shown.push(target);
                }
            }
        }
    }
    let notes = package
        .parts()
        .filter(|part| content_type(part).is_some_and(|kind| NOTES.contains(&kind)));
    let stories = shown
        .iter()
        .filter_map(|name| package.part(name))
        .chain(notes);
    for part in stories {
        scan_part(&part, &limits, &mut found, None)
            .map_err(|error| format!("{}: {error}", part.name()))?;
    }
    Ok(Scanned {
        kinds: KINDS
            .iter()
            .filter(|(name, _)| found.contains(name))
            .map(|(name, _)| (*name).to_owned())
            .collect(),
        stories_not_shown: STORY_NAMES
            .iter()
            .filter(|(kind, which, _)| !shown_types.contains(&(*kind, *which)))
            .map(|(_, _, story)| (*story).to_owned())
            .collect(),
    })
}

/// The body's sections in order.
#[derive(Default)]
struct Sections(Vec<SectionRefs>);

/// The headers and footers a section names, as (`header` or `footer`, `w:type`,
/// relationship id), and whether it has a title page.
#[derive(Default)]
struct SectionRefs {
    references: Vec<(String, String, String)>,
    title_page: bool,
}

/// The relationship types of the settings part.
const SETTINGS: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/settings",
];

/// Whether the settings turn on even-page headers and footers (`w:evenAndOddHeaders`).
fn even_and_odd_headers(main: &Part, limits: &Limits) -> Result<bool, String> {
    let Some(settings) = main
        .relationships()
        .iter()
        .filter(|rel| SETTINGS.contains(&rel.relationship_type()))
        .find_map(|rel| rel.target_part())
        .and_then(|name| main.package().part(&name))
    else {
        return Ok(false);
    };
    let bytes = settings.read().map_err(|error| error.to_string())?;
    let input = Input::decode(&bytes).map_err(|error| error.to_string())?;
    let mut reader = input.reader(limits);
    let names = Names::new(&mut reader);
    while let Some(event) = reader.read_event().map_err(|error| error.to_string())? {
        if let Event::Start(tag) = event {
            if names.is_w(&tag.name()) && tag.name().local() == "evenAndOddHeaders" {
                return Ok(on(w_value(&tag, &names, "val")));
            }
        }
    }
    Ok(false)
}

const W: [&str; 2] = [
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
];
const A: [&str; 2] = [
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
];
const WP: [&str; 2] = [
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
];
const M: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "http://purl.oclc.org/ooxml/officeDocument/math",
];
const R: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];
const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPC: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas";
const V: &str = "urn:schemas-microsoft-com:vml";
const W10: &str = "urn:schemas-microsoft-com:office:word";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const O: &str = "urn:schemas-microsoft-com:office:office";
const CHARTS: [&str; 3] = [
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://purl.oclc.org/ooxml/drawingml/chart",
    "http://schemas.microsoft.com/office/drawing/2014/chartex",
];
const DIAGRAMS: [&str; 2] = [
    "http://schemas.openxmlformats.org/drawingml/2006/diagram",
    "http://purl.oclc.org/ooxml/drawingml/diagram",
];

/// The VML elements that draw a shape.
const VML_SHAPES: [&str; 8] = [
    "shape",
    "rect",
    "roundrect",
    "oval",
    "line",
    "polyline",
    "arc",
    "curve",
];

/// The picture formats the layout draws.
const DRAWN: [&str; 5] = [
    "image/png",
    "image/x-png",
    "image/jpeg",
    "image/jpg",
    "image/pjpeg",
];

/// The text directions of horizontal lines (`w:textDirection`): Transitional `lrTb`, Strict
/// `tb`, and those with East Asian characters turned, `lrTbV` and `tbV`.
const HORIZONTAL: [&str; 4] = ["lrTb", "tb", "lrTbV", "tbV"];

/// The namespaces a part's names are compared with, Transitional and Strict.
struct Names {
    w: [NsId; 2],
    a: [NsId; 2],
    wp: [NsId; 2],
    m: [NsId; 2],
    r: [NsId; 2],
    wps: NsId,
    wpg: NsId,
    wpc: NsId,
    v: NsId,
    w10: NsId,
    mc: NsId,
    o: NsId,
}

impl Names {
    fn new(reader: &mut Reader) -> Self {
        let mut both = |uris: [&str; 2]| [reader.intern(uris[0]), reader.intern(uris[1])];
        let (w, a, wp, m, r) = (both(W), both(A), both(WP), both(M), both(R));
        Names {
            w,
            a,
            wp,
            m,
            r,
            wps: reader.intern(WPS),
            wpg: reader.intern(WPG),
            wpc: reader.intern(WPC),
            v: reader.intern(V),
            w10: reader.intern(W10),
            mc: reader.intern(MC),
            o: reader.intern(O),
        }
    }

    fn is_w(&self, name: &Name) -> bool {
        in_any(name, &self.w)
    }
}

/// Whether a name is in one of the namespaces.
fn in_any(name: &Name, namespaces: &[NsId]) -> bool {
    namespaces.iter().any(|ns| name.namespace() == Some(*ns))
}

/// An attribute of a tag in one of the namespaces.
fn attribute<'t>(tag: &'t StartTag, namespaces: &[NsId], local: &str) -> Option<&'t str> {
    namespaces
        .iter()
        .find_map(|ns| tag.attribute(Some(*ns), local))
}

/// An open element the scan keeps: what the kinds that depend on its content need.
enum Open {
    /// A DrawingML shape: whether it holds a text box, and whether it is turned.
    Shape { text: bool, turned: bool },
    /// A VML shape outside an embedded object: whether it holds a text box or a picture,
    /// and whether its style turns it.
    Vml {
        text: bool,
        picture: bool,
        turned: bool,
        /// Its `id` and `o:spid`, which a box that is in no chain names as its next.
        names: [Option<String>; 2],
    },
    /// An embedded object: whether it shows a VML preview picture or a drawing.
    Object { preview: bool, drawing: bool },
    /// The paragraphs of a text box.
    TextBox,
    /// A section's properties.
    Section,
    /// The columns of a section, with the w:col elements counted.
    Columns(usize),
    /// A section's page borders.
    PageBorders,
    /// Anything else.
    Other,
}

/// What a start tag tells the scan to do.
enum Step {
    /// Keep the element open.
    Open(Open),
    /// Skip it with all it holds.
    Skip,
}

fn scan_part(
    part: &Part,
    limits: &Limits,
    found: &mut BTreeSet<&'static str>,
    mut sections: Option<&mut Sections>,
) -> Result<(), String> {
    let bytes = part.read().map_err(|error| error.to_string())?;
    let input = Input::decode(&bytes).map_err(|error| error.to_string())?;
    let mut reader = input.reader(limits);
    let names = Names::new(&mut reader);
    let mut open: Vec<Open> = Vec::new();
    while let Some(event) = reader.read_event().map_err(|error| error.to_string())? {
        match event {
            Event::Start(tag) => {
                let step = if skipped(&tag, &names) {
                    Step::Skip
                } else {
                    start(
                        &tag,
                        &names,
                        part,
                        &mut open,
                        found,
                        sections.as_deref_mut(),
                    )
                };
                match step {
                    Step::Open(element) => open.push(element),
                    Step::Skip => reader.skip_element().map_err(|error| error.to_string())?,
                }
            }
            Event::End(_) => {
                if let Some(element) = open.pop() {
                    end(element, found);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the layout leaves an element out with all it holds: deleted text, the
/// formatting a revision replaced (w:pPrChange and the like), the fallbacks of Markup
/// Compatibility, and VML shape types, which only define shapes.
fn skipped(tag: &StartTag, names: &Names) -> bool {
    let name = tag.name();
    let local = name.local();
    if names.is_w(&name) {
        matches!(local, "del" | "moveFrom") || local.ends_with("Change")
    } else {
        name.is(names.mc, "Fallback") || name.is(names.v, "shapetype")
    }
}

/// Whether an on/off attribute is on: absent, or not 0, false or off.
fn on(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "false" | "off"))
}

/// The value of attribute `local` of `tag` in WordprocessingML's namespace.
fn w_value<'t>(tag: &'t StartTag, names: &Names, local: &str) -> Option<&'t str> {
    attribute(tag, &names.w, local)
}

/// The value of a property of a VML style (`rotation:90;mso-next-textbox:#t2`).
fn style_property<'s>(style: &'s str, property: &str) -> Option<&'s str> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        (name.trim().eq_ignore_ascii_case(property)).then(|| value.trim())
    })
}

/// Whether a rotation, in DrawingML's units of 1/60,000 degree or VML's degrees (a number
/// with an optional unit such as `fd`), turns its object.
fn turns(value: &str) -> bool {
    let number: String = value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        .collect();
    number.parse::<f64>().is_ok_and(|turn| turn != 0.0)
}

/// Whether the part a relationship id names is a picture in a format the layout does not
/// draw.
fn undrawn_picture(part: &Part, id: &str) -> bool {
    let Some(target) = part
        .relationships()
        .get(id)
        .and_then(|rel| rel.target_part())
    else {
        return false;
    };
    let Some(kind) = part
        .package()
        .part(&target)
        .and_then(|picture| picture.content_type())
    else {
        return false;
    };
    let kind = kind.split(';').next().unwrap_or_default().trim();
    kind.starts_with("image/") && !DRAWN.contains(&kind.to_ascii_lowercase().as_str())
}

/// What a start tag shows, and the element to keep open for it.
fn start(
    tag: &StartTag,
    names: &Names,
    part: &Part,
    open: &mut [Open],
    found: &mut BTreeSet<&'static str>,
    sections: Option<&mut Sections>,
) -> Step {
    let name = tag.name();
    let local = name.local();
    let in_object = open
        .iter()
        .any(|element| matches!(element, Open::Object { .. }));
    let in_text_box = open.iter().any(|element| matches!(element, Open::TextBox));
    if names.is_w(&name) {
        let parent_section = matches!(open.last(), Some(Open::Section));
        match local {
            "sectPr" => {
                if let Some(sections) = sections {
                    sections.0.push(SectionRefs::default());
                }
                return Step::Open(Open::Section);
            }
            "headerReference" | "footerReference" if parent_section => {
                let id = attribute(tag, &names.r, "id");
                let section = sections.and_then(|sections| sections.0.last_mut());
                if let (Some(section), Some(id)) = (section, id) {
                    let kind = local.trim_end_matches("Reference");
                    let which = w_value(tag, names, "type").unwrap_or("default");
                    section
                        .references
                        .push((kind.to_owned(), which.to_owned(), id.to_owned()));
                }
            }
            "titlePg" if parent_section => {
                let section = sections.and_then(|sections| sections.0.last_mut());
                if let Some(section) = section.filter(|_| on(w_value(tag, names, "val"))) {
                    section.title_page = true;
                }
            }
            "cols" if parent_section => {
                let num = w_value(tag, names, "num").and_then(|num| num.parse::<u32>().ok());
                if num.is_some_and(|num| num > 1) {
                    found.insert("columns");
                }
                return Step::Open(Open::Columns(0));
            }
            "col" => {
                if let Some(Open::Columns(count)) = open.last_mut() {
                    *count += 1;
                }
            }
            "docGrid" if parent_section => {
                if matches!(
                    w_value(tag, names, "type"),
                    Some("lines" | "linesAndChars" | "snapToChars")
                ) {
                    found.insert("document grid");
                }
            }
            // As the translation: what Word does without w:countBy is not known.
            "lnNumType" if parent_section => {
                let count = w_value(tag, names, "countBy").and_then(|n| n.parse::<u32>().ok());
                if count.is_some_and(|count| count > 0) {
                    found.insert("line numbers");
                }
            }
            "pgBorders" if parent_section => return Step::Open(Open::PageBorders),
            "top" | "left" | "bottom" | "right" | "start" | "end"
                if matches!(open.last(), Some(Open::PageBorders)) =>
            {
                if !matches!(w_value(tag, names, "val"), Some("nil" | "none")) {
                    found.insert("page borders");
                }
            }
            "vAlign" if parent_section => {
                if matches!(
                    w_value(tag, names, "val"),
                    Some("center" | "both" | "bottom")
                ) {
                    found.insert("vertical alignment");
                }
            }
            "framePr" => {
                found.insert("text frame");
            }
            "tblpPr" => {
                found.insert("floating table");
            }
            "textDirection" => {
                if w_value(tag, names, "val").is_some_and(|value| !HORIZONTAL.contains(&value)) {
                    found.insert("vertical text");
                }
            }
            // A section's w:bidi turns its page setup, not its text.
            "bidi" | "rtl" | "bidiVisual" if !parent_section => {
                if on(w_value(tag, names, "val")) {
                    found.insert("right-to-left text");
                }
            }
            "ruby" => {
                found.insert("ruby");
            }
            "fitText" => {
                found.insert("text fitting");
            }
            "tcFitText" => {
                if on(w_value(tag, names, "val")) {
                    found.insert("text fitting");
                }
            }
            "object" => {
                return Step::Open(Open::Object {
                    preview: false,
                    drawing: false,
                });
            }
            // The translation leaves an object shown by a drawing out whole.
            "drawing" if in_object => {
                if let Some(Open::Object { drawing, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Object { .. }))
                {
                    *drawing = true;
                }
                return Step::Skip;
            }
            "txbxContent" => return Step::Open(Open::TextBox),
            "tbl" if in_text_box => {
                found.insert("text box content");
            }
            _ => {}
        }
    } else if in_any(&name, &names.a) {
        match local {
            "graphicData" => match tag.attribute(None, "uri") {
                Some(uri) if CHARTS.contains(&uri) => {
                    found.insert("chart");
                }
                Some(uri) if DIAGRAMS.contains(&uri) => {
                    found.insert("SmartArt");
                }
                _ => {}
            },
            "blip" => {
                if attribute(tag, &names.r, "embed").is_some_and(|id| undrawn_picture(part, id)) {
                    found.insert("picture format");
                }
            }
            "xfrm" if tag.attribute(None, "rot").is_some_and(turns) => {
                if let Some(Open::Shape { turned, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Shape { .. }))
                {
                    *turned = true;
                }
            }
            _ => {}
        }
    } else if name.namespace() == Some(names.wps) {
        match local {
            "wsp" => {
                return Step::Open(Open::Shape {
                    text: false,
                    turned: false,
                });
            }
            "txbx" | "linkedTxbx" => {
                if local == "linkedTxbx" {
                    found.insert("linked text box");
                }
                if let Some(Open::Shape { text, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Shape { .. }))
                {
                    *text = true;
                }
            }
            "bodyPr" if !matches!(tag.attribute(None, "vert"), Some("horz") | None) => {
                found.insert("vertical text");
            }
            _ => {}
        }
    } else if name.is(names.wpg, "wgp") || name.is(names.wpc, "wpc") {
        // The translation leaves a group or a canvas out whole, with what it holds.
        found.insert("shape");
        return Step::Skip;
    } else if in_any(&name, &names.wp) {
        match local {
            "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom" => {
                found.insert("text wrapping");
            }
            "anchor" if in_text_box => {
                found.insert("text box content");
            }
            _ => {}
        }
    } else if name.is(names.w10, "wrap") {
        if matches!(
            tag.attribute(None, "type"),
            Some("square" | "tight" | "through" | "topAndBottom")
        ) {
            found.insert("text wrapping");
        }
    } else if in_any(&name, &names.m) {
        if matches!(local, "oMath" | "oMathPara") {
            found.insert("equation");
        }
    } else if name.namespace() == Some(names.v) {
        if local == "group" {
            found.insert("shape");
            return Step::Skip;
        } else if VML_SHAPES.contains(&local) {
            if in_object {
                return Step::Open(Open::Other);
            }
            let style = tag.attribute(None, "style").unwrap_or_default();
            return Step::Open(Open::Vml {
                text: false,
                picture: false,
                turned: style_property(style, "rotation").is_some_and(turns),
                names: [
                    tag.attribute(None, "id").map(str::to_owned),
                    tag.attribute(Some(names.o), "spid").map(str::to_owned),
                ],
            });
        } else if local == "textbox" {
            let style = tag.attribute(None, "style").unwrap_or_default();
            // A box that names itself as the next is in no chain, as Word writes some
            // (business/report-hhs-usability-test-report.docx); officewright-doc reads it so.
            let shape = open.iter().rev().find_map(|element| match element {
                Open::Vml { names, .. } => Some(names),
                _ => None,
            });
            let next = style_property(style, "mso-next-textbox")
                .map(|next| next.strip_prefix('#').unwrap_or(next))
                .filter(|next| !next.is_empty());
            if next.is_some_and(|next| {
                !shape.is_some_and(|names| names.iter().flatten().any(|name| name == next))
            }) {
                found.insert("linked text box");
            }
            if style_property(style, "layout-flow").is_some_and(|flow| flow.starts_with("vertical"))
            {
                found.insert("vertical text");
            }
            if let Some(Open::Vml { text, .. }) = open
                .iter_mut()
                .rev()
                .find(|e| matches!(e, Open::Vml { .. }))
            {
                *text = true;
            }
        } else if local == "imagedata" {
            if attribute(tag, &names.r, "id").is_some_and(|id| undrawn_picture(part, id)) {
                found.insert("picture format");
            }
            for element in open.iter_mut().rev() {
                match element {
                    Open::Vml { picture, .. } => {
                        *picture = true;
                        break;
                    }
                    Open::Object { preview, .. } => {
                        *preview = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    Step::Open(Open::Other)
}

/// What an element shows once its content is read.
fn end(element: Open, found: &mut BTreeSet<&'static str>) {
    match element {
        Open::Shape { text, turned } => {
            if !text {
                found.insert("shape");
            } else if turned {
                found.insert("rotated text box");
            }
        }
        Open::Vml {
            text,
            picture,
            turned,
            ..
        } => {
            if text && turned {
                found.insert("rotated text box");
            } else if !text && !picture {
                found.insert("shape");
            }
        }
        Open::Object { preview, drawing } => {
            if drawing || !preview {
                found.insert("embedded object");
            }
        }
        Open::Columns(count) if count > 1 => {
            found.insert("columns");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A ZIP archive of `files`, stored without compression.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let (crc, size, length) = (crc32(data), data.len() as u32, name.len() as u16);
            let header = |signature: u32, central: bool| {
                let mut h = signature.to_le_bytes().to_vec();
                if central {
                    h.extend(20u16.to_le_bytes());
                }
                h.extend(20u16.to_le_bytes());
                h.extend([0u8; 6]); // flags, method (stored), time
                h.extend(0x21u16.to_le_bytes()); // date
                h.extend(crc.to_le_bytes());
                h.extend(size.to_le_bytes());
                h.extend(size.to_le_bytes());
                h.extend(length.to_le_bytes());
                h.extend(0u16.to_le_bytes());
                if central {
                    h.extend([0u8; 6]); // comment, disk, internal attributes
                    h.extend(0u32.to_le_bytes());
                    h.extend(offset.to_le_bytes());
                }
                h.extend(name.as_bytes());
                h
            };
            out.extend(header(0x0403_4b50, false));
            out.extend(*data);
            central.extend(header(0x0201_4b50, true));
        }
        let (start, count) = (out.len() as u32, files.len() as u16);
        let size = central.len() as u32;
        out.extend(central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(count.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    const NAMESPACES: &str = concat!(
        r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
        r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
        r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
        r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" "#,
        r#"xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" "#,
        r#"xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" "#,
        r#"xmlns:v="urn:schemas-microsoft-com:vml" "#,
        r#"xmlns:w10="urn:schemas-microsoft-com:office:word" "#,
        r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" "#,
        r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
        r#"xmlns:o="urn:schemas-microsoft-com:office:office" "#,
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
    );

    /// A Word document whose body is `body`, with three headers (header1 to header3.xml,
    /// relationships rId3 to rId5) holding `headers`, settings `settings`, a styles part
    /// `styles` and pictures rId1 (EMF) and rId2 (PNG). The body's sections name the headers
    /// they show.
    fn package(body: &str, headers: [&str; 3], settings: &str, styles: &str) -> Vec<u8> {
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        package_with(body, headers, settings, styles, &format!("{rel}/settings"))
    }

    /// As [`package`], the settings part named by a relationship of type `settings_type`.
    fn package_with(
        body: &str,
        headers: [&str; 3],
        settings: &str,
        styles: &str,
        settings_type: &str,
    ) -> Vec<u8> {
        let main = "application/vnd.openxmlformats-officedocument.wordprocessingml";
        let types = format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="emf" ContentType="image/x-emf"/>
<Default Extension="png" ContentType="image/png"/>
<Override PartName="/word/document.xml" ContentType="{main}.document.main+xml"/>
<Override PartName="/word/header1.xml" ContentType="{main}.header+xml"/>
<Override PartName="/word/header2.xml" ContentType="{main}.header+xml"/>
<Override PartName="/word/header3.xml" ContentType="{main}.header+xml"/>
<Override PartName="/word/settings.xml" ContentType="{main}.settings+xml"/>
<Override PartName="/word/styles.xml" ContentType="{main}.styles+xml"/></Types>"#
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let document_rels = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/image" Target="media/image1.emf"/><Relationship Id="rId2" Type="{rel}/image" Target="media/image2.png"/><Relationship Id="rId3" Type="{rel}/header" Target="header1.xml"/><Relationship Id="rId4" Type="{rel}/header" Target="header2.xml"/><Relationship Id="rId5" Type="{rel}/header" Target="header3.xml"/><Relationship Id="rId6" Type="{settings_type}" Target="settings.xml"/></Relationships>"#
        );
        let story = |root: &str, content: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:{root} {NAMESPACES}>{content}</w:{root}>"#
            )
        };
        let body = story("document", &format!("<w:body>{body}</w:body>"));
        let [one, two, three] = headers.map(|content| story("hdr", content));
        let settings = story("settings", settings);
        let styles = story("styles", styles);
        zip(&[
            ("[Content_Types].xml", types.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("word/document.xml", body.as_bytes()),
            ("word/_rels/document.xml.rels", document_rels.as_bytes()),
            ("word/header1.xml", one.as_bytes()),
            ("word/header2.xml", two.as_bytes()),
            ("word/header3.xml", three.as_bytes()),
            ("word/settings.xml", settings.as_bytes()),
            ("word/styles.xml", styles.as_bytes()),
            ("word/media/image1.emf", b"EMF"),
            ("word/media/image2.png", b"PNG"),
        ])
    }

    /// A document whose body is `body`, its section showing header1.xml, which holds
    /// `header`.
    fn document(body: &str, header: &str, styles: &str) -> Vec<u8> {
        let body = format!(
            r#"{body}<w:sectPr><w:headerReference w:type="default" r:id="rId3"/></w:sectPr>"#
        );
        package(&body, [header, "", ""], "", styles)
    }

    fn kinds(body: &str) -> Vec<String> {
        scan_bytes(document(body, "", "")).unwrap().kinds
    }

    fn graphic(uri: &str, content: &str) -> String {
        format!(
            r#"<w:p><w:r><w:drawing><wp:inline><a:graphic><a:graphicData uri="{uri}">{content}</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        )
    }

    fn anchored(wrap: &str) -> String {
        format!(
            r#"<w:p><w:r><w:drawing><wp:anchor>{wrap}<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><a:blip r:embed="rId2"/></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>"#
        )
    }

    fn shape(content: &str) -> String {
        graphic(
            "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
            &format!("<wps:wsp>{content}</wps:wsp>"),
        )
    }

    const TEXT: &str = "<wps:txbx><w:txbxContent><w:p/></w:txbxContent></wps:txbx>";

    fn section(content: &str) -> String {
        format!("<w:p/><w:sectPr>{content}</w:sectPr>")
    }

    fn pict(content: &str) -> String {
        format!("<w:p><w:r><w:pict>{content}</w:pict></w:r></w:p>")
    }

    /// Each kind, from the smallest markup that holds it.
    #[test]
    fn every_kind_is_found() {
        let cases: Vec<(String, &str)> = vec![
            (graphic("http://schemas.openxmlformats.org/drawingml/2006/chart", ""), "chart"),
            (
                graphic("http://schemas.microsoft.com/office/drawing/2014/chartex", ""),
                "chart",
            ),
            (
                graphic("http://schemas.openxmlformats.org/drawingml/2006/diagram", ""),
                "SmartArt",
            ),
            (shape("<wps:spPr/>"), "shape"),
            (
                graphic(
                    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
                    "<wpg:wgp/>",
                ),
                "shape",
            ),
            (pict(r#"<v:rect style="width:10pt"/>"#), "shape"),
            (pict(r#"<v:line from="0,0" to="10,0"/>"#), "shape"),
            (pict("<v:group><v:shape/></v:group>"), "shape"),
            ("<w:p><w:r><w:object/></w:r></w:p>".to_owned(), "embedded object"),
            (
                "<w:p><w:r><w:object><w:drawing/></w:object></w:r></w:p>".to_owned(),
                "embedded object",
            ),
            ("<w:p><m:oMathPara><m:oMath/></m:oMathPara></w:p>".to_owned(), "equation"),
            (
                graphic(
                    "http://schemas.openxmlformats.org/drawingml/2006/picture",
                    r#"<a:blip r:embed="rId1"/>"#,
                ),
                "picture format",
            ),
            (
                pict(r#"<v:shape><v:imagedata r:id="rId1"/></v:shape>"#),
                "picture format",
            ),
            (section(r#"<w:cols w:num="2"/>"#), "columns"),
            (
                section(r#"<w:cols w:equalWidth="0"><w:col w:w="10"/><w:col w:w="10"/></w:cols>"#),
                "columns",
            ),
            (section(r#"<w:docGrid w:type="lines" w:linePitch="360"/>"#), "document grid"),
            (section(r#"<w:lnNumType w:countBy="1"/>"#), "line numbers"),
            (
                section(r#"<w:pgBorders><w:top w:val="single"/></w:pgBorders>"#),
                "page borders",
            ),
            (section(r#"<w:vAlign w:val="center"/>"#), "vertical alignment"),
            (r#"<w:p><w:pPr><w:framePr w:dropCap="drop"/></w:pPr></w:p>"#.to_owned(), "text frame"),
            (
                r#"<w:tbl><w:tblPr><w:tblpPr w:tblpY="10"/></w:tblPr></w:tbl>"#.to_owned(),
                "floating table",
            ),
            (anchored("<wp:wrapSquare/>"), "text wrapping"),
            (anchored("<wp:wrapTopAndBottom/>"), "text wrapping"),
            (pict(r#"<v:rect><v:textbox/><w10:wrap type="square"/></v:rect>"#), "text wrapping"),
            (
                r#"<w:tbl><w:tr><w:tc><w:tcPr><w:textDirection w:val="btLr"/></w:tcPr></w:tc></w:tr></w:tbl>"#
                    .to_owned(),
                "vertical text",
            ),
            (shape(&format!(r#"<wps:bodyPr vert="vert270"/>{TEXT}"#)), "vertical text"),
            (
                pict(r#"<v:rect><v:textbox style="layout-flow:vertical"/></v:rect>"#),
                "vertical text",
            ),
            (
                shape(&format!(r#"<wps:spPr><a:xfrm rot="5400000"/></wps:spPr>{TEXT}"#)),
                "rotated text box",
            ),
            (
                pict(r#"<v:rect style="rotation:90"><v:textbox/></v:rect>"#),
                "rotated text box",
            ),
            (shape(r#"<wps:linkedTxbx id="1" seq="1"/>"#), "linked text box"),
            (
                pict(
                    r##"<v:rect id="t1" o:spid="s1"><v:textbox style="mso-next-textbox:#t2"/></v:rect>"##,
                ),
                "linked text box",
            ),
            (
                shape("<wps:txbx><w:txbxContent><w:tbl/></w:txbxContent></wps:txbx>"),
                "text box content",
            ),
            (
                shape(&format!(
                    "<wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx>",
                    anchored("")
                )),
                "text box content",
            ),
            (r#"<w:p><w:pPr><w:bidi/></w:pPr></w:p>"#.to_owned(), "right-to-left text"),
            (
                r#"<w:p><w:r><w:rPr><w:rtl w:val="1"/></w:rPr></w:r></w:p>"#.to_owned(),
                "right-to-left text",
            ),
            (r#"<w:p><w:r><w:ruby/></w:r></w:p>"#.to_owned(), "ruby"),
            (
                r#"<w:p><w:r><w:rPr><w:fitText w:val="720"/></w:rPr></w:r></w:p>"#.to_owned(),
                "text fitting",
            ),
            (
                r#"<w:tbl><w:tr><w:tc><w:tcPr><w:tcFitText/></w:tcPr></w:tc></w:tr></w:tbl>"#
                    .to_owned(),
                "text fitting",
            ),
        ];
        let mut seen = BTreeSet::new();
        for (body, kind) in &cases {
            assert_eq!(kinds(body), [*kind], "{body}");
            seen.insert(*kind);
        }
        let all: BTreeSet<&str> = KINDS.iter().map(|(name, _)| *name).collect();
        assert_eq!(seen, all, "every kind has a case");
    }

    /// What the layout lays out, or leaves out as Word does, is no such content.
    #[test]
    fn what_the_layout_lays_out_is_not_found() {
        let cases: Vec<String> = vec![
            // A picture in PNG, inline and anchored without wrapping; a basic text box.
            graphic(
                "http://schemas.openxmlformats.org/drawingml/2006/picture",
                r#"<a:blip r:embed="rId2"/>"#,
            ),
            anchored("<wp:wrapNone/>"),
            shape(TEXT),
            shape(&format!(r#"<wps:bodyPr vert="horz"/><wps:spPr><a:xfrm rot="0"/></wps:spPr>{TEXT}"#)),
            pict(r#"<v:rect style="rotation:0"><v:textbox style="layout-flow:horizontal"/></v:rect>"#),
            pict(r#"<v:shape><v:imagedata r:id="rId2"/></v:shape>"#),
            // A turned picture is not a turned text box; wrapping in front of the text.
            pict(r#"<v:shape style="rotation:90"><v:imagedata r:id="rId2"/><w10:wrap type="none"/></v:shape>"#),
            // An embedded object shown by its preview picture; a shape type defines only.
            r#"<w:p><w:r><w:object><v:shapetype/><v:shape><v:imagedata r:id="rId2"/></v:shape></w:object></w:r></w:p>"#.to_owned(),
            pict("<v:shapetype><v:path/></v:shapetype>"),
            // A box that names itself as its next one, by its id or its o:spid.
            pict(r##"<v:rect id="t1"><v:textbox style="mso-next-textbox:#t1"/></v:rect>"##),
            pict(
                r##"<v:rect o:spid="_x0000_s2050"><v:textbox style="mso-next-textbox:#_x0000_s2050"/></v:rect>"##,
            ),
            pict(r##"<v:rect><v:textbox style="mso-next-textbox:"/></v:rect>"##),
            // One column, the default grid and line numbering off, page borders of none.
            section(r#"<w:cols w:num="1" w:space="720"/><w:docGrid w:type="default"/><w:lnNumType w:countBy="0"/><w:pgBorders><w:top w:val="none"/><w:left w:val="nil"/></w:pgBorders><w:vAlign w:val="top"/>"#),
            section(r#"<w:cols><w:col w:w="10"/></w:cols>"#),
            // Line numbers without w:countBy, which the translation does not report either,
            // a section's right-to-left page setup, and borders of paragraphs and tables.
            section(r#"<w:lnNumType w:restart="newPage"/><w:bidi/>"#),
            r#"<w:p><w:pPr><w:pBdr><w:top w:val="single"/></w:pBdr></w:pPr></w:p>"#.to_owned(),
            r#"<w:tbl><w:tblPr><w:tblBorders><w:top w:val="single"/><w:left w:val="single"/></w:tblBorders></w:tblPr></w:tbl>"#.to_owned(),
            r#"<w:tbl><w:tr><w:tc><w:tcPr><w:textDirection w:val="lrTbV"/></w:tcPr></w:tc></w:tr></w:tbl>"#.to_owned(),
            // A cell's vertical alignment; text from left to right.
            r#"<w:tbl><w:tr><w:tc><w:tcPr><w:vAlign w:val="center"/><w:textDirection w:val="lrTb"/><w:tcFitText w:val="0"/></w:tcPr></w:tc></w:tr></w:tbl>"#.to_owned(),
            r#"<w:p><w:pPr><w:bidi w:val="0"/></w:pPr><w:r><w:rPr><w:rtl w:val="false"/></w:rPr></w:r></w:p>"#.to_owned(),
            // Deleted content, the formatting a revision replaced, a fallback.
            format!("<w:p><w:del>{}</w:del></w:p>", "<w:r><w:object/></w:r>"),
            r#"<w:p><w:pPr><w:pPrChange><w:pPr><w:framePr w:w="10"/></w:pPr></w:pPrChange></w:pPr></w:p>"#.to_owned(),
            section(r#"<w:sectPrChange><w:sectPr><w:cols w:num="2"/></w:sectPr></w:sectPrChange>"#),
            format!(
                "<w:p><w:r><mc:AlternateContent><mc:Choice Requires=\"wps\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r></w:p>",
                "<w:drawing/>",
                "<w:pict><v:rect/></w:pict>"
            ),
        ];
        for body in &cases {
            assert_eq!(kinds(body), Vec::<String>::new(), "{body}");
        }
    }

    /// The stories are read, headers too, and the styles are not.
    #[test]
    fn headers_are_read_and_styles_are_not() {
        let chart = graphic("http://schemas.openxmlformats.org/drawingml/2006/chart", "");
        assert_eq!(
            scan_bytes(document("<w:p/>", &chart, "")).unwrap().kinds,
            ["chart"]
        );
        let framed = r#"<w:style w:styleId="Frame"><w:pPr><w:framePr w:w="10"/></w:pPr></w:style>"#;
        assert!(
            scan_bytes(document("<w:p/>", "", framed))
                .unwrap()
                .kinds
                .is_empty()
        );
        // Kinds come in the order of KINDS, each once.
        let both = format!("{}{}{}", section(r#"<w:cols w:num="3"/>"#), chart, chart);
        assert_eq!(kinds(&both), ["chart", "columns"]);
    }

    /// Headers of a type Word does not show are not read: first-page ones without a title
    /// page, even-page ones without w:evenAndOddHeaders, and those no section names.
    #[test]
    fn only_the_headers_word_shows_are_read() {
        let chart = graphic("http://schemas.openxmlformats.org/drawingml/2006/chart", "");
        let equation = "<w:p><m:oMath/></w:p>";
        let ruby = "<w:p><w:r><w:ruby/></w:r></w:p>";
        let headers = [chart.as_str(), equation, ruby];
        let body = |title: bool| {
            format!(
                r#"<w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId3"/><w:headerReference w:type="first" r:id="rId4"/><w:headerReference w:type="even" r:id="rId5"/>{}</w:sectPr>"#,
                if title { "<w:titlePg/>" } else { "" }
            )
        };
        let scan = |title: bool, settings: &str| {
            scan_bytes(package(&body(title), headers, settings, ""))
                .unwrap()
                .kinds
        };
        assert_eq!(scan(false, ""), ["chart"]);
        assert_eq!(scan(true, ""), ["chart", "equation"]);
        let off = body(false).replace("</w:sectPr>", r#"<w:titlePg w:val="0"/></w:sectPr>"#);
        assert_eq!(
            scan_bytes(package(&off, headers, "", "")).unwrap().kinds,
            ["chart"],
            "a title page turned off"
        );
        assert_eq!(scan(false, "<w:evenAndOddHeaders/>"), ["chart", "ruby"]);
        assert_eq!(
            scan(false, r#"<w:evenAndOddHeaders w:val="0"/>"#),
            ["chart"]
        );
        assert_eq!(
            scan(false, r#"<w:titlePg/><w:evenAndOddHeaders w:val="false"/>"#),
            ["chart"],
            "a title page is a section's"
        );
        // A title page of a later section shows the first-page header too.
        let two = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="first" r:id="rId4"/></w:sectPr></w:pPr></w:p><w:p/><w:sectPr><w:titlePg/></w:sectPr>"#;
        assert_eq!(
            scan_bytes(package(two, headers, "", "")).unwrap().kinds,
            ["equation"]
        );
        // A title page is a section's own: section 2, without one, names a first-page
        // header no page shows, while section 1's title page shows no first-page header.
        let own = r#"<w:p><w:pPr><w:sectPr><w:titlePg/></w:sectPr></w:pPr></w:p><w:p/><w:sectPr><w:headerReference w:type="first" r:id="rId4"/></w:sectPr>"#;
        let scanned = scan_bytes(package(own, headers, "", "")).unwrap();
        assert!(scanned.kinds.is_empty(), "{scanned:?}");
        assert!(
            scanned
                .stories_not_shown
                .contains(&"first-page-header".to_owned())
        );
        // A section's own header of a type takes the place of the one before: both
        // sections' headers are shown.
        let replaced = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId3"/></w:sectPr></w:pPr></w:p><w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId4"/></w:sectPr>"#;
        assert_eq!(
            scan_bytes(package(replaced, headers, "", ""))
                .unwrap()
                .kinds,
            ["chart", "equation"]
        );
        // The settings part may be named by Strict's relationship type.
        let strict_settings = scan_bytes(package_with(
            &body(false),
            headers,
            "<w:evenAndOddHeaders/>",
            "",
            "http://purl.oclc.org/ooxml/officeDocument/relationships/settings",
        ))
        .unwrap();
        assert_eq!(strict_settings.kinds, ["chart", "ruby"]);
        // No section names a header: none is read.
        assert!(
            scan_bytes(package("<w:p/>", headers, "", ""))
                .unwrap()
                .kinds
                .is_empty()
        );
    }

    /// Strict documents name the same content in other namespaces.
    #[test]
    fn strict_names_are_read_as_transitional_ones() {
        let strict = r#"xmlns:sw="http://purl.oclc.org/ooxml/wordprocessingml/main" xmlns:sa="http://purl.oclc.org/ooxml/drawingml/main" xmlns:swp="http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing" xmlns:sm="http://purl.oclc.org/ooxml/officeDocument/math" xmlns:sr="http://purl.oclc.org/ooxml/officeDocument/relationships""#;
        let cases = [
            (
                r#"<sw:p><sw:r><sw:drawing><swp:inline><sa:graphic><sa:graphicData uri="http://purl.oclc.org/ooxml/drawingml/chart"/></sa:graphic></swp:inline></sw:drawing></sw:r></sw:p>"#,
                vec!["chart"],
            ),
            (
                r#"<sw:p><sw:r><sw:drawing><swp:inline><sa:graphic><sa:graphicData uri="http://purl.oclc.org/ooxml/drawingml/diagram"/></sa:graphic></swp:inline></sw:drawing></sw:r></sw:p>"#,
                vec!["SmartArt"],
            ),
            (
                r#"<sw:p><sw:r><sw:drawing><swp:anchor><swp:wrapSquare/><sa:graphic><sa:graphicData uri="x"><sa:blip sr:embed="rId1"/></sa:graphicData></sa:graphic></swp:anchor></sw:drawing></sw:r></sw:p>"#,
                vec!["picture format", "text wrapping"],
            ),
            (r#"<sw:p><sm:oMath/></sw:p>"#, vec!["equation"]),
            (
                r#"<sw:tbl><sw:tr><sw:tc><sw:tcPr><sw:textDirection sw:val="tb"/></sw:tcPr></sw:tc></sw:tr></sw:tbl>"#,
                vec![],
            ),
            (
                r#"<sw:tbl><sw:tr><sw:tc><sw:tcPr><sw:textDirection sw:val="tbV"/></sw:tcPr></sw:tc></sw:tr></sw:tbl>"#,
                vec![],
            ),
            (
                r#"<sw:tbl><sw:tr><sw:tc><sw:tcPr><sw:textDirection sw:val="rl"/></sw:tcPr></sw:tc></sw:tr></sw:tbl>"#,
                vec!["vertical text"],
            ),
            (
                r#"<sw:tbl><sw:tr><sw:tc><sw:tcPr><sw:textDirection sw:val="lr"/></sw:tcPr></sw:tc></sw:tr></sw:tbl>"#,
                vec!["vertical text"],
            ),
        ];
        for (content, expected) in cases {
            let body = format!(r#"<w:p {strict}>{content}</w:p>"#);
            assert_eq!(kinds(&body), expected, "{content}");
        }
    }

    /// The stories of Word's formatting export of the kinds and types no section shows,
    /// through the sections in order.
    #[test]
    fn the_stories_of_types_no_section_shows_are_named() {
        let not_shown = |body: &str, settings: &str| {
            scan_bytes(package(body, ["", "", ""], settings, ""))
                .unwrap()
                .stories_not_shown
        };
        let refs = r#"<w:headerReference w:type="default" r:id="rId3"/><w:headerReference w:type="first" r:id="rId4"/><w:footerReference w:type="even" r:id="rId5"/>"#;
        assert_eq!(
            not_shown(&format!("<w:p/><w:sectPr>{refs}</w:sectPr>"), ""),
            [
                "first-page-header",
                "even-pages-header",
                "primary-footer",
                "first-page-footer",
                "even-pages-footer"
            ]
        );
        assert_eq!(
            not_shown(
                &format!("<w:p/><w:sectPr>{refs}<w:titlePg/></w:sectPr>"),
                "<w:evenAndOddHeaders/>"
            ),
            ["even-pages-header", "primary-footer", "first-page-footer"]
        );
        assert_eq!(not_shown("<w:p/>", "").len(), 6);
    }

    /// What a group of shapes, a canvas or an object shown by a drawing holds is left out
    /// with it.
    #[test]
    fn what_objects_left_out_whole_hold_is_not_read() {
        let canvas = graphic(
            "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas",
            "<wpc:wpc><wps:wsp><wps:txbx><w:txbxContent><w:tbl/></w:txbxContent></wps:txbx></wps:wsp></wpc:wpc>",
        );
        assert_eq!(kinds(&canvas), ["shape"]);
        let object = format!(
            "<w:p><w:r><w:object>{}</w:object></w:r></w:p>",
            r#"<w:drawing><wp:inline><a:graphic><a:graphicData uri="x"><a:blip r:embed="rId1"/></a:graphicData></a:graphic></wp:inline></w:drawing>"#
        );
        assert_eq!(kinds(&object), ["embedded object"]);
        // A drawing beside a preview picture still leaves the object out.
        let both = pict("").replace(
            "<w:pict></w:pict>",
            r#"<w:object><v:shape><v:imagedata r:id="rId2"/></v:shape><w:drawing/></w:object>"#,
        );
        assert_eq!(kinds(&both), ["embedded object"]);
    }

    /// What a group of shapes holds is left out with it: only the group is found.
    #[test]
    fn the_content_of_groups_is_not_read() {
        let in_group = shape("<wps:txbx><w:txbxContent><w:tbl/></w:txbxContent></wps:txbx>")
            .replace("<wps:wsp>", "<wpg:wgp><wps:wsp>")
            .replace("</wps:wsp>", "</wps:wsp></wpg:wgp>");
        assert_eq!(kinds(&in_group), ["shape"]);
        let vml = pict(
            r#"<v:group><v:shape><v:imagedata r:id="rId1"/><v:textbox style="mso-next-textbox:#b"/></v:shape></v:group>"#,
        );
        assert_eq!(kinds(&vml), ["shape"]);
    }

    #[test]
    fn damaged_markup_is_an_error() {
        let error = scan_bytes(document("<w:p>", "", "")).unwrap_err();
        assert!(error.starts_with("/word/document.xml: "), "{error}");
        assert!(scan_bytes(b"not a zip".to_vec()).is_err());
        assert!(scan(std::path::Path::new("/nonexistent/a.docx")).is_err());
    }

    /// Each kind is named by the messages the layout and the translation write for it at
    /// the time of writing, and not by messages about something else that share a word.
    #[test]
    fn each_kind_is_named_by_its_own_messages_only() {
        let named = [
            (
                "chart",
                "a drawing that shows no picture (a shape, chart or group) is left out",
            ),
            (
                "SmartArt",
                "a drawing that shows no picture (a shape, chart or group) is left out",
            ),
            (
                "shape",
                "a VML drawing that shows no picture or text box (a shape or group) is left out (w:pict)",
            ),
            (
                "shape",
                "a group of VML shapes is left out, with the text boxes in it (w:pict)",
            ),
            (
                "embedded object",
                "an embedded object without a preview picture is left out (w:object)",
            ),
            ("equation", "an equation is left out"),
            (
                "picture format",
                "pictures in emf format are drawn as grey boxes",
            ),
            ("columns", "text in columns is laid out in one column"),
            (
                "document grid",
                "the document grid (w:docGrid) is not followed, here and in every other section that has one",
            ),
            (
                "document grid",
                "spacing in line units on a document grid keeps its value in points",
            ),
            (
                "line numbers",
                "line numbers (w:lnNumType) are not drawn, here and in every other section that has them",
            ),
            (
                "page borders",
                "page borders (w:pgBorders) are not drawn, here and in every other section that has them",
            ),
            (
                "text frame",
                "a text frame (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
            ),
            (
                "text frame",
                "a drop cap (w:framePr) is laid out as an ordinary paragraph, here and wherever else the document has one",
            ),
            (
                "floating table",
                "floating tables are laid out in the text where they are anchored",
            ),
            (
                "vertical alignment",
                "the body text of a section is laid out from the top of its pages, whatever its vertical alignment",
            ),
            (
                "text wrapping",
                "text does not wrap around floating objects yet: it runs over or under them",
            ),
            (
                "text wrapping",
                "floating objects in table cells are not drawn yet",
            ),
            (
                "vertical text",
                "vertical text in table cells is laid out horizontally",
            ),
            (
                "vertical text",
                "vertical text (w:textDirection) is laid out horizontally, here and in every other section that has it",
            ),
            (
                "vertical text",
                "vertical text in a text box (wps:bodyPr vert, or layout-flow in VML) is drawn across, here and wherever else the document has some",
            ),
            (
                "rotated text box",
                "a turned text box (a:xfrm rot, or rotation in VML) is drawn upright, here and wherever else the document has one",
            ),
            (
                "linked text box",
                "a chain of linked text boxes (wps:linkedTxbx, or mso-next-textbox in VML) is drawn with all its text in its first box",
            ),
            ("text box content", "tables in text boxes are not drawn yet"),
            (
                "text box content",
                "floating objects in text boxes are not drawn",
            ),
            (
                "right-to-left text",
                "right-to-left text is laid out from left to right, its characters in the order they are stored",
            ),
            (
                "right-to-left text",
                "a right-to-left paragraph (w:bidi) is laid out left to right, here and wherever else the document has one",
            ),
            (
                "right-to-left text",
                "right-to-left tables are laid out left to right",
            ),
            (
                "text fitting",
                "the cell's text fitting (w:tcFitText) is not applied",
            ),
        ];
        for (kind, message) in named {
            assert!(names(message, words(kind).unwrap()), "{kind}: {message}");
        }
        let others = [
            (
                "vertical text",
                "rows that a vertically merged cell ties together are not split across pages",
            ),
            (
                "rotated text box",
                "cropped, turned or mirrored pictures are drawn whole and upright",
            ),
            (
                "picture format",
                "a picture is drawn as a grey box: the PNG data is damaged",
            ),
            (
                "columns",
                "the columns of a text box (numCol) are drawn as one, here and wherever else the document has them",
            ),
            (
                "text box content",
                "a text box shaped other than as a rectangle (a:prstGeom other than rect) is drawn as a rectangle",
            ),
            ("text wrapping", "paragraph borders are not drawn yet"),
            (
                "shape",
                "a text box shaped other than as a rectangle (a:prstGeom other than rect) is drawn as a rectangle",
            ),
            (
                "line numbers",
                "list numbers in a format the layout does not write are written in decimal",
            ),
        ];
        for (kind, message) in others {
            assert!(!names(message, words(kind).unwrap()), "{kind}: {message}");
        }
    }

    #[test]
    fn entries_name_content_by_their_words() {
        let wrapping = words("text wrapping").unwrap();
        assert!(names(
            "Text does not WRAP around floating objects yet",
            wrapping
        ));
        assert!(names(
            "floating objects in table cells are not drawn yet",
            wrapping
        ));
        assert!(!names("paragraph borders are not drawn yet", wrapping));
        let frames = words("text frame").unwrap();
        assert!(names(
            "a drop cap (w:framePr) is laid out as an ordinary paragraph",
            frames
        ));
        assert!(names(
            "a text frame (w:framePr) is laid out as an ordinary paragraph",
            frames
        ));
        assert_eq!(words("no such kind"), None);
        for (name, words) in KINDS {
            assert!(!words.is_empty(), "{name}");
            for word in words {
                assert_eq!(*word, word.to_lowercase(), "{name}");
            }
        }
    }
}
