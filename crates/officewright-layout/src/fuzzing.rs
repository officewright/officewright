//! Entry points for the fuzz targets in `fuzz/`, the examples and the tests that use the
//! repository's test fonts: the layout's own code, reached without a whole document.
//! They are not part of the API and change with the code they reach.

use officewright_core::Pt;
use officewright_font::{FontBook, FontFace};

use crate::fonts::Fonts;
use crate::ir::{Document, Inline, Ligatures, RunStyle};
use crate::itemize::{Item, itemize, shape_runs};
use crate::metrics::{self, TypoRule};
use crate::report::{LayoutReport, Location};
use crate::shape::{Features, ShapeRequest, ShapedRun, Shaper, ShapingFace};
use crate::slot::{self, Slot};
use crate::symbol::SymbolFont;

/// A glyph as [`shape`] places it, in the face's design units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    id: u16,
    cluster: usize,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
    unsafe_to_break: bool,
}

impl Glyph {
    /// The glyph's id in the face.
    pub fn id(self) -> u16 {
        self.id
    }

    /// The byte offset in the text of the first character the glyph stands for.
    pub fn cluster(self) -> usize {
        self.cluster
    }

    /// How far the glyph moves the pen.
    pub fn x_advance(self) -> i32 {
        self.x_advance
    }

    /// How far the glyph is drawn right of the pen.
    pub fn x_offset(self) -> i32 {
        self.x_offset
    }

    /// How far the glyph is drawn above the pen.
    pub fn y_offset(self) -> i32 {
        self.y_offset
    }

    /// Whether breaking the text at this glyph's cluster could change the shaping.
    pub fn unsafe_to_break(self) -> bool {
        self.unsafe_to_break
    }
}

/// The result of [`shape`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shaped {
    glyphs: Vec<Glyph>,
    nominal: bool,
    replaced: usize,
}

impl Shaped {
    /// The glyphs, left to right.
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// Whether the text was shaped from the character map alone, because the face's
    /// layout tables could not be read.
    pub fn is_nominal(&self) -> bool {
        self.nominal
    }

    /// How many glyphs the face's layout tables named that the face does not have,
    /// replaced by the missing-glyph shape.
    pub fn replaced_glyphs(&self) -> usize {
        self.replaced
    }
}

/// How [`shape`] shapes a text: the features a run's formatting asks for, and the
/// script and language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    features: Features,
    script: Option<[u8; 4]>,
    language: Option<String>,
}

impl Settings {
    /// Kerning off, no ligatures or contextual alternates, the script taken from the
    /// text, no language.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets whether the text is kerned.
    pub fn with_kerning(mut self, kerning: bool) -> Self {
        self.features.kerning = kerning;
        self
    }

    /// Sets the ligatures.
    pub fn with_ligatures(mut self, ligatures: Ligatures) -> Self {
        self.features.ligatures = ligatures;
        self
    }

    /// Sets whether contextual alternates are used.
    pub fn with_contextual_alternates(mut self, on: bool) -> Self {
        self.features.contextual_alternates = on;
        self
    }

    /// Sets the script, as an ISO 15924 code such as `Latn`; a code that is not four
    /// letters leaves the script to be taken from the text.
    pub fn with_script(mut self, code: &str) -> Self {
        self.script = <[u8; 4]>::try_from(code.as_bytes()).ok();
        self
    }

    /// Sets the language, as a BCP 47 tag such as `ro`.
    pub fn with_language(mut self, tag: &str) -> Self {
        self.language = Some(tag.to_owned());
        self
    }

    fn request(&self) -> ShapeRequest {
        ShapeRequest {
            features: self.features,
            script: self.script,
            language: self.language.clone(),
        }
    }
}

/// Shapes `text` in `face`, left to right, as the layout shapes a run.
pub fn shape(face: &FontFace, text: &str, settings: &Settings) -> Shaped {
    let face = ShapingFace::new(face.clone());
    let run = Shaper::new().shape(&face, text, &settings.request());
    Shaped {
        glyphs: run
            .glyphs()
            .iter()
            .map(|glyph| Glyph {
                id: glyph.glyph.to_u16(),
                cluster: glyph.cluster,
                x_advance: glyph.x_advance,
                x_offset: glyph.x_offset,
                y_offset: glyph.y_offset,
                unsafe_to_break: glyph.unsafe_to_break,
            })
            .collect(),
        nominal: run.is_nominal(),
        replaced: run.replaced_glyphs(),
    }
}

/// Shapes each request, a face of `faces` by its index, a text and settings, in turn with
/// one shaper, as the layout shapes the runs of a document: what one run leaves in the
/// shaper's caches must not change how another is shaped.
pub fn shape_in_turn(faces: &[FontFace], requests: &[(usize, &str, Settings)]) -> Vec<Shaped> {
    let faces: Vec<ShapingFace> = faces.iter().cloned().map(ShapingFace::new).collect();
    let mut shaper = Shaper::new();
    requests
        .iter()
        .filter_map(|(face, text, settings)| {
            let face = faces.get(*face)?;
            let run = shaper.shape(face, text, &settings.request());
            Some(Shaped {
                glyphs: glyphs(&run),
                nominal: run.is_nominal(),
                replaced: run.replaced_glyphs(),
            })
        })
        .collect()
}

/// Shapes each text in `face` with kerning, ligatures and contextual alternates off and
/// on, and once more with a script and a language given, and checks what the layout
/// relies on: every glyph is one the face has, and the clusters are character
/// boundaries of the text that never go backwards.
///
/// # Errors
///
/// What was wrong, naming the text, the settings and the glyph.
pub fn check_shaping(face: &FontFace, texts: &[&str]) -> Result<(), String> {
    let shaping_face = ShapingFace::new(face.clone());
    let mut shaper = Shaper::new();
    let every = Ligatures::default()
        .with_standard(true)
        .with_contextual(true)
        .with_historical(true)
        .with_discretional(true);
    let mut settings = Vec::new();
    for kerning in [false, true] {
        for (ligatures, alternates) in [(Ligatures::default(), false), (every, true)] {
            settings.push(
                Settings::new()
                    .with_kerning(kerning)
                    .with_ligatures(ligatures)
                    .with_contextual_alternates(alternates),
            );
        }
    }
    // An explicit script, right or wrong for the text, and a language with forms of its
    // own reach the script and language systems of the font's layout tables.
    settings.push(Settings::new().with_script("Latn").with_language("ro"));
    settings.push(Settings::new().with_script("Arab").with_language("ar"));
    for text in texts {
        for settings in &settings {
            let run = shaper.shape(&shaping_face, text, &settings.request());
            let mut previous = 0;
            for (index, glyph) in run.glyphs().iter().enumerate() {
                let at = || format!("{text:?} with {settings:?}, glyph {index}");
                if glyph.glyph.to_u16() >= face.glyph_count() {
                    return Err(format!(
                        "{}: glyph {} does not exist",
                        at(),
                        glyph.glyph.to_u16()
                    ));
                }
                if !text.is_char_boundary(glyph.cluster) {
                    return Err(format!(
                        "{}: cluster {} is not a character boundary",
                        at(),
                        glyph.cluster
                    ));
                }
                if glyph.cluster < previous {
                    return Err(format!(
                        "{}: cluster {} goes backwards",
                        at(),
                        glyph.cluster
                    ));
                }
                previous = glyph.cluster;
            }
        }
    }
    Ok(())
}

/// A run of a paragraph as [`paragraph`] itemizes and shapes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    text: String,
    source: String,
    face: Option<String>,
    size: f64,
    synthetic_bold: bool,
    synthetic_italic: bool,
    script: String,
    language: Option<String>,
    glyphs: Vec<Glyph>,
    glyph_count: u16,
    line: Option<LineHeight>,
}

impl Run {
    /// The line metrics of the face at the run's size; `None` without a face.
    pub fn line(&self) -> Option<LineHeight> {
        self.line
    }

    /// The text as drawn: capitals where the formatting asks, symbols as Unicode.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text as the document has it.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The family name of the face, `None` without one.
    pub fn face(&self) -> Option<&str> {
        self.face.as_deref()
    }

    /// The size in points.
    pub fn size(&self) -> f64 {
        self.size
    }

    /// Whether bold is drawn by stroking the outlines.
    pub fn synthetic_bold(&self) -> bool {
        self.synthetic_bold
    }

    /// Whether italic is drawn by slanting the glyphs.
    pub fn synthetic_italic(&self) -> bool {
        self.synthetic_italic
    }

    /// The script, as an ISO 15924 code.
    pub fn script(&self) -> &str {
        &self.script
    }

    /// The language, as a BCP 47 tag.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// The glyphs, in the face's design units; clusters are byte offsets of
    /// [`text`](Self::text).
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }
}

/// A paragraph as [`paragraph`] itemizes and shapes it.
#[derive(Clone, Debug)]
pub struct Paragraph {
    text: String,
    runs: Vec<Run>,
    others: usize,
    report: LayoutReport,
}

impl Paragraph {
    /// The paragraph's text as drawn.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The runs of text, in order.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// How many pieces are not text: tabs, breaks, objects, note references.
    pub fn others(&self) -> usize {
        self.others
    }

    /// What the fonts did that differs from Word.
    pub fn report(&self) -> &LayoutReport {
        &self.report
    }
}

/// Itemizes and shapes the `inlines` of a paragraph of `document` with the fonts of
/// `book`, as the layout does.
pub fn paragraph(book: &FontBook, document: &Document, inlines: &[Inline]) -> Paragraph {
    let mut fonts = Fonts::new(book);
    let mut shaper = Shaper::new();
    let location = Location::default();
    let itemized = itemize(document, inlines, &mut fonts, &location);
    let shaped = shape_runs(document, &itemized, &mut fonts, &mut shaper, &location);
    let text_runs = itemized.items.iter().filter_map(|item| match item {
        Item::Text(run) => Some(run),
        _ => None,
    });
    let runs = text_runs
        .zip(&shaped)
        .map(|(run, shaped)| {
            let face = run.face.and_then(|id| fonts.face(id));
            Run {
                text: itemized
                    .text
                    .get(run.range.clone())
                    .unwrap_or_default()
                    .to_owned(),
                source: itemized
                    .source
                    .get(run.source.clone())
                    .unwrap_or_default()
                    .to_owned(),
                face: face
                    .as_ref()
                    .and_then(|face| face.face().names().family())
                    .map(str::to_owned),
                size: run.size.0,
                synthetic_bold: run.synthetic_bold,
                synthetic_italic: run.synthetic_italic,
                script: String::from_utf8_lossy(&run.script).into_owned(),
                language: run.language.clone(),
                glyphs: shaped
                    .as_ref()
                    .map_or_else(Vec::new, |shaped| glyphs(shaped)),
                glyph_count: face.as_ref().map_or(0, |face| face.face().glyph_count()),
                line: face
                    .as_ref()
                    .map(|face| line_metrics(face.face(), run.size.0, TypoMetrics::default())),
            }
        })
        .collect();
    let others = itemized
        .items
        .iter()
        .filter(|item| !matches!(item, Item::Text(_)))
        .count();
    Paragraph {
        text: itemized.text,
        runs,
        others,
        report: fonts.into_report(),
    }
}

fn glyphs(run: &ShapedRun) -> Vec<Glyph> {
    run.glyphs()
        .iter()
        .map(|glyph| Glyph {
            id: glyph.glyph.to_u16(),
            cluster: glyph.cluster,
            x_advance: glyph.x_advance,
            x_offset: glyph.x_offset,
            y_offset: glyph.y_offset,
            unsafe_to_break: glyph.unsafe_to_break,
        })
        .collect()
}

/// Itemizes and shapes a paragraph as [`paragraph`] does and checks what the layout
/// relies on: the runs are the paragraph's text in order, each a whole number of
/// characters and at most 4096 bytes, none but the first starting with a mark or joiner;
/// their source text follows in order and covers the paragraph's; every glyph is one its
/// face has; clusters are character boundaries of the run's text that never go
/// backwards.
///
/// # Errors
///
/// What was wrong, naming the run and the glyph.
pub fn check_paragraph(
    book: &FontBook,
    document: &Document,
    inlines: &[Inline],
) -> Result<(), String> {
    let mut fonts = Fonts::new(book);
    let itemized = itemize(document, inlines, &mut fonts, &Location::default());
    // The source text: each run's part follows the one before, and together they are
    // all of it.
    let mut source_end = 0;
    let mut previous: Option<&crate::itemize::TextRun> = None;
    for (index, item) in itemized.items.iter().enumerate() {
        let Item::Text(run) = item else {
            previous = None;
            continue;
        };
        // A run that follows another without an item between starts with a character
        // that starts a cluster, unless the one before was cut for its length and had
        // no cluster start to cut at.
        let text = itemized.text.get(run.range.clone()).unwrap_or_default();
        let cut =
            previous.is_some_and(|before| before.range.len() + 4 > crate::itemize::MAX_RUN_BYTES);
        if previous.is_some() && !cut && text.starts_with(crate::itemize::attaches) {
            return Err(format!(
                "item {index} starts with a mark or joiner: {text:?}"
            ));
        }
        previous = Some(run);
        if run.source.start != source_end || itemized.source.get(run.source.clone()).is_none() {
            return Err(format!(
                "item {index}: source {:?} after {source_end} of {} bytes",
                run.source,
                itemized.source.len()
            ));
        }
        source_end = run.source.end;
    }
    if source_end != itemized.source.len() {
        return Err(format!(
            "the runs' source ends at {source_end} of {} bytes",
            itemized.source.len()
        ));
    }
    let paragraph = paragraph(book, document, inlines);
    let joined: String = paragraph.runs.iter().map(|run| run.text.as_str()).collect();
    if joined != paragraph.text {
        return Err(format!(
            "the runs {joined:?} are not the paragraph's text {:?}",
            paragraph.text
        ));
    }
    for (index, run) in paragraph.runs.iter().enumerate() {
        if run.text.is_empty() || run.text.len() > crate::itemize::MAX_RUN_BYTES {
            return Err(format!("run {index} has {} bytes", run.text.len()));
        }
        let mut previous = 0;
        for (number, glyph) in run.glyphs.iter().enumerate() {
            if glyph.id >= run.glyph_count {
                return Err(format!(
                    "run {index}, glyph {number}: glyph {} does not exist",
                    glyph.id
                ));
            }
            if !run.text.is_char_boundary(glyph.cluster) || glyph.cluster < previous {
                return Err(format!(
                    "run {index}, glyph {number}: cluster {} of {:?}",
                    glyph.cluster, run.text
                ));
            }
            previous = glyph.cluster;
        }
    }
    Ok(())
}

/// When the typographic metrics of a font replace its Windows ones, for
/// [`line_metrics`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TypoMetrics {
    /// For TrueType fonts that set `USE_TYPO_METRICS`, as the layout does.
    #[default]
    TrueTypeOnly,
    /// For every font that sets the flag.
    Always,
    /// Never.
    Never,
}

/// A face's line metrics at a size, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineHeight {
    /// Above the baseline.
    pub ascent: f64,
    /// Below the baseline.
    pub descent: f64,
    /// The external leading.
    pub leading: f64,
    /// The height of a single-spaced line: the three together.
    pub height: f64,
}

/// The line metrics of `face` at `size` points, as the layout takes them: from the
/// Windows metrics, or the typographic ones where `typo` says so.
pub fn line_metrics(face: &FontFace, size: f64, typo: TypoMetrics) -> LineHeight {
    let rule = match typo {
        TypoMetrics::TrueTypeOnly => TypoRule::TrueTypeOnly,
        TypoMetrics::Always => TypoRule::Always,
        TypoMetrics::Never => TypoRule::Never,
    };
    let metrics = metrics::line_metrics(face, Pt(size), rule);
    LineHeight {
        ascent: metrics.ascent.0,
        descent: metrics.descent.0,
        leading: metrics.leading.0,
        height: metrics.height().0,
    }
}

/// Which of a run's fonts `ch` takes in `style`: `ascii`, `hAnsi`, `eastAsia` or `cs`.
pub fn font_slot(ch: char, style: &RunStyle) -> &'static str {
    match slot::slot(ch, style) {
        Slot::Ascii => "ascii",
        Slot::HighAnsi => "hAnsi",
        Slot::EastAsia => "eastAsia",
        Slot::ComplexScript => "cs",
    }
}

/// The Unicode character the layout draws for `code` of the symbol font `font` (Symbol
/// or Wingdings) when the font is not installed.
pub fn symbol_unicode(font: &str, code: u32) -> Option<char> {
    SymbolFont::of(font)?.unicode(code)
}
