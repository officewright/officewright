//! Entry points for the fuzz targets in `fuzz/`, the examples and the tests that use the
//! repository's test fonts: the layout's own code, reached without a whole document.
//! They are not part of the API and change with the code they reach.

use officewright_core::Pt;
use officewright_font::{FontBook, FontFace};

use crate::fonts::Fonts;
use crate::ir::{Document, Inline, Ligatures, RunStyle};
use crate::itemize::{Item, TextRun, itemize, shape_runs};
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
/// characters and at most 4096 bytes; a run that follows another of the same formatting,
/// face and script, cut for its length, starts where HarfRust starts a cluster (it draws
/// no dotted circle before it), and one that follows another run does not start with a
/// mark or joiner; their source text follows in order and covers the paragraph's; every
/// glyph is one its face has; clusters are character boundaries of the run's text that
/// never go backwards.
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
    let location = Location::default();
    let itemized = itemize(document, inlines, &mut fonts, &location);
    let mut shaper = Shaper::new();
    let shaped = shape_runs(document, &itemized, &mut fonts, &mut shaper, &location);
    let mut shaped = shaped.iter().map(Option::as_deref);
    // The source text: each run's part follows the one before, and together they are
    // all of it.
    let mut source_end = 0;
    let mut previous: Option<(&TextRun, Option<&ShapedRun>)> = None;
    for (index, item) in itemized.items.iter().enumerate() {
        let Item::Text(run) = item else {
            previous = None;
            continue;
        };
        let this = shaped.next().flatten();
        let text = itemized.text.get(run.range.clone()).unwrap_or_default();
        if let Some((before, before_shaped)) = previous {
            let cut = before.face == run.face
                && before.style == run.style
                && before.size == run.size
                && before.formatted_size == run.formatted_size
                && before.synthetic_bold == run.synthetic_bold
                && before.synthetic_italic == run.synthetic_italic
                && before.script == run.script
                && before.language == run.language;
            if cut {
                // A run cut for its length starts where a cluster starts, so HarfRust
                // puts no dotted circle before it; unless the run before is one cluster,
                // with nowhere to be cut.
                let one_cluster = before_shaped
                    .is_none_or(|before| before.glyphs().iter().all(|glyph| glyph.cluster == 0));
                let dotted_circle = run
                    .face
                    .and_then(|id| fonts.face(id))
                    .and_then(|face| face.face().glyph_id('\u{25CC}'));
                let first = this.and_then(|shaped| shaped.glyphs().first());
                let starts_with_circle = first
                    .is_some_and(|glyph| glyph.cluster == 0 && Some(glyph.glyph) == dotted_circle);
                if starts_with_circle && !one_cluster && !text.starts_with('\u{25CC}') {
                    return Err(format!(
                        "item {index}, cut for its length, starts inside a cluster: {:?}",
                        text.chars().take(8).collect::<String>()
                    ));
                }
            } else if text.starts_with(crate::itemize::attaches) {
                // A mark or joiner stays in the run of the character before it.
                return Err(format!(
                    "item {index} starts with a mark or joiner: {text:?}"
                ));
            }
        }
        previous = Some((run, this));
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

/// A word of a line as [`lay_out`] places it: what has no space in it, between spaces,
/// tabs and the pieces that are not text.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    text: String,
    start: f64,
    end: f64,
}

impl Word {
    /// The text as drawn.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where its first glyph starts, in points from the start edge of the column.
    pub fn start(&self) -> f64 {
        self.start
    }

    /// Where its last glyph ends.
    pub fn end(&self) -> f64 {
        self.end
    }
}

/// A line of a paragraph as [`lay_out`] breaks it.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    text: String,
    words: Vec<Word>,
    tabs: Vec<(f64, f64)>,
    width: f64,
    ascent: f64,
    descent: f64,
    height: f64,
    baseline: f64,
    end: &'static str,
}

impl Line {
    /// The text the line holds as drawn, a tab as `\t`.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The words, in order.
    pub fn words(&self) -> &[Word] {
        &self.words
    }

    /// Where each tab starts and ends, in points from the start edge of the column.
    pub fn tabs(&self) -> &[(f64, f64)] {
        &self.tabs
    }

    /// The width of what is on the line, without the spaces that hang at its end.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// How far its text reaches above the baseline.
    pub fn ascent(&self) -> f64 {
        self.ascent
    }

    /// How far its text reaches below the baseline.
    pub fn descent(&self) -> f64 {
        self.descent
    }

    /// Its height, after the paragraph's line spacing.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Its baseline, from its top.
    pub fn baseline(&self) -> f64 {
        self.baseline
    }

    /// Why it ends: `wrap`, `break`, `page break`, `column break` or `paragraph`.
    pub fn end(&self) -> &'static str {
        self.end
    }
}

/// A paragraph as [`lay_out`] breaks it into lines.
#[derive(Clone, Debug)]
pub struct LaidOut {
    lines: Vec<Line>,
    height: f64,
    report: LayoutReport,
}

impl LaidOut {
    /// The height of all its lines.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// The lines, in order.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// What the layout did that differs from Word.
    pub fn report(&self) -> &LayoutReport {
        &self.report
    }
}

/// Lays out `paragraph` of `document` in a column `width` points wide, with the fonts
/// of `book`, as the layout does.
pub fn lay_out(
    book: &FontBook,
    document: &Document,
    paragraph: &crate::ir::Paragraph,
    width: f64,
) -> LaidOut {
    use crate::lines::{LineEnd, PieceKind};

    let mut fonts = Fonts::new(book);
    let mut shaper = Shaper::new();
    let location = Location::default();
    let layout = crate::paragraph::lay_out(
        document,
        paragraph,
        Pt(width),
        &mut fonts,
        &mut shaper,
        &location,
    );
    let itemized = &layout.itemized;
    let lines = layout
        .lines
        .iter()
        .map(|line| {
            let mut words: Vec<Word> = Vec::new();
            let mut tabs = Vec::new();
            let mut text = String::new();
            // Whether the next glyph that is not a space starts a word.
            let mut new_word = true;
            for piece in &line.pieces {
                match &piece.kind {
                    PieceKind::Text { .. } | PieceKind::Hyphen { .. } => {
                        let hyphen = matches!(piece.kind, PieceKind::Hyphen { .. });
                        let glyphs = piece.glyphs(itemized);
                        for (index, glyph) in glyphs.iter().enumerate() {
                            let cluster = if hyphen {
                                "-".to_owned()
                            } else {
                                let end = glyphs
                                    .iter()
                                    .skip(index + 1)
                                    .find(|next| next.cluster != glyph.cluster)
                                    .map_or_else(
                                        || piece_text_end(&piece.kind),
                                        |next| next.cluster,
                                    );
                                if glyphs
                                    .get(index.wrapping_sub(1))
                                    .is_some_and(|before| before.cluster == glyph.cluster)
                                {
                                    String::new()
                                } else {
                                    itemized
                                        .text
                                        .get(glyph.cluster..end)
                                        .unwrap_or_default()
                                        .to_owned()
                                }
                            };
                            text.push_str(&cluster);
                            if cluster.chars().all(char::is_whitespace) && !cluster.is_empty() {
                                new_word = true;
                                continue;
                            }
                            let (start, end) = (glyph.x.0, glyph.x.0 + glyph.advance.0);
                            match words.last_mut() {
                                Some(word) if !new_word => {
                                    word.text.push_str(&cluster);
                                    word.end = word.end.max(end);
                                }
                                _ => words.push(Word {
                                    text: cluster,
                                    start,
                                    end,
                                }),
                            }
                            new_word = false;
                        }
                    }
                    PieceKind::Tab { .. } => {
                        text.push('\t');
                        tabs.push((piece.x.0, piece.x.0 + piece.width.0));
                        new_word = true;
                    }
                    PieceKind::Break { .. } | PieceKind::Object { .. } => new_word = true,
                }
            }
            Line {
                text,
                words,
                tabs,
                width: line.width.0,
                ascent: line.ascent.0,
                descent: line.descent.0,
                height: line.height.0,
                baseline: line.baseline.0,
                end: match line.end {
                    LineEnd::Wrap => "wrap",
                    LineEnd::Break => "break",
                    LineEnd::PageBreak => "page break",
                    LineEnd::ColumnBreak => "column break",
                    LineEnd::Paragraph => "paragraph",
                },
            }
        })
        .collect();
    LaidOut {
        lines,
        height: layout.height().0,
        report: fonts.into_report(),
    }
}

/// The space between a paragraph with `above` of `document` and the one with `below`
/// that follows it.
pub fn space_between(
    document: &Document,
    above: &crate::ir::ParagraphProps,
    below: &crate::ir::ParagraphProps,
) -> f64 {
    crate::paragraph::space_between(document, above, below).0
}

/// Where the drawn text of a text piece ends.
fn piece_text_end(kind: &crate::lines::PieceKind) -> usize {
    match kind {
        crate::lines::PieceKind::Text { text, .. } => text.end,
        _ => 0,
    }
}

/// Lays out `paragraph` of `document` in a column `width` points wide with the fonts of
/// `book`, and checks what pagination relies on: the lines hold the paragraph's text in
/// order, all of it, each byte once; a line wider than its room is one the breaker
/// marks as holding a piece too wide for any line, and the report says so; a line
/// starts with a soft hyphen only where a line may end before it; heights and baselines
/// are numbers, heights and widths not negative.
pub fn check_lines(
    book: &FontBook,
    document: &Document,
    paragraph: &crate::ir::Paragraph,
    width: f64,
) -> Result<(), String> {
    let mut fonts = Fonts::new(book);
    let mut shaper = Shaper::new();
    let location = Location::default();
    let layout = crate::paragraph::lay_out(
        document,
        paragraph,
        Pt(width),
        &mut fonts,
        &mut shaper,
        &location,
    );
    let report = fonts.into_report();
    let reported = report
        .iter()
        .any(|entry| entry.kind() == crate::ReportKind::Overflow);
    let indent = paragraph.props().indent();
    let (text, _) = crate::lines::break_text(&layout.itemized);
    let breaks = crate::breaks::BreakMap::new(&text, paragraph.props().word_wrap());
    let mut end = 0;
    for (index, line) in layout.lines.iter().enumerate() {
        if line.range.start != end {
            return Err(format!(
                "line {index} starts at {} after {end}",
                line.range.start
            ));
        }
        end = line.range.end;
        let room = width
            - indent.start().0
            - indent.end().0
            - if index == 0 {
                indent.first_line().0
            } else {
                0.0
            };
        let wide = line.width.0 > room.max(0.0) + 1e-6 && line.width.0 > 0.0;
        if wide && !(line.overflow && reported) {
            return Err(format!(
                "line {index} is {} wide in {room}: {:?}",
                line.width.0,
                text.get(line.range.clone())
            ));
        }
        // A soft hyphen starts a line only where a line may end before it, never where
        // a cut moved back before it.
        let starts_with_soft_hyphen = text
            .get(line.range.clone())
            .is_some_and(|line| line.starts_with('\u{00AD}'));
        if index > 0 && starts_with_soft_hyphen && breaks.at(line.range.start).is_none() {
            return Err(format!(
                "line {index} starts with a soft hyphen it may not start with"
            ));
        }
        let numbers = [
            line.height.0,
            line.baseline.0,
            line.ascent.0,
            line.descent.0,
            line.width.0,
        ];
        if numbers.iter().any(|number| !number.is_finite())
            || line.height.0 < 0.0
            || line.width.0 < -1e-9
        {
            return Err(format!("line {index}: {numbers:?}"));
        }
    }
    if layout.lines.is_empty() {
        return Err("no line".to_owned());
    }
    if end != text.len() {
        return Err(format!("the lines end at {end} of {} bytes", text.len()));
    }
    Ok(())
}

/// Paragraphs laid out and drawn by [`paint`]: what the display list holds, one line
/// per item, and what the layout reported.
#[derive(Clone, Debug)]
pub struct Painted {
    text: String,
    report: LayoutReport,
}

impl Painted {
    /// The display list as text, positions and sizes in twips from the column's top
    /// left corner:
    ///
    /// - `glyphs FACE SIZE #RRGGBB @X,Y [bold] [italic] [scale S] "TEXT" ID@X[,Y] ...`
    ///   where `FACE` is the family, then the face's style unless it is Regular
    ///   (`Carlito Bold`), `bold` and `italic` are synthetic, and glyph places are from
    ///   the run's origin on its baseline;
    /// - `rect @X,Y WxH #RRGGBB`;
    /// - `line X,Y X,Y WIDTH STYLE #RRGGBB`;
    /// - `image N @X,Y WxH`.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What the layout did that differs from Word.
    pub fn report(&self) -> &LayoutReport {
        &self.report
    }
}

/// Lays out `paragraphs` of `document` one under the other, with the space between
/// them, in a column `width` points wide with the fonts of `book`, and draws them.
pub fn paint(
    book: &FontBook,
    document: &Document,
    paragraphs: &[crate::ir::Paragraph],
    width: f64,
) -> Painted {
    use std::fmt::Write as _;

    use crate::display::{DisplayItem, LineStyle, Painting, Point};

    let mut fonts = Fonts::new(book);
    let mut shaper = Shaper::new();
    let location = Location::default();
    let mut items = Vec::new();
    let mut top = 0.0;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        // The space above the first paragraph is its own; between two, the larger.
        let previous = index.checked_sub(1).and_then(|i| paragraphs.get(i));
        top += match previous {
            Some(previous) => {
                crate::paragraph::space_between(document, previous.props(), paragraph.props())
            }
            None => crate::paragraph::space_before(document, paragraph.props()),
        }
        .0;
        let layout = crate::paragraph::lay_out(
            document,
            paragraph,
            Pt(width),
            &mut fonts,
            &mut shaper,
            &location,
        );
        let mut painting = Painting {
            document,
            paragraph,
            layout: &layout,
            fonts: &mut fonts,
            location: &location,
            background: None,
        };
        let origin = Point {
            x: Pt(0.0),
            y: Pt(top),
        };
        painting.paint(0..layout.lines.len(), origin, &mut items);
        top += layout.height().0;
    }
    let twips = |pt: Pt| (pt.0 * 20.0).round() as i64;
    let hex = |color: officewright_core::Color| {
        format!(
            "#{:02X}{:02X}{:02X}",
            color.red(),
            color.green(),
            color.blue()
        )
    };
    let mut text = String::new();
    for item in &items {
        let _ = match item {
            DisplayItem::Glyphs(run) => {
                let family = fonts
                    .face(run.face)
                    .map(|face| {
                        let names = face.face().names();
                        let family = names.family().unwrap_or_default();
                        match names.subfamily() {
                            Some(style) if style != "Regular" => format!("{family} {style}"),
                            _ => family.to_owned(),
                        }
                    })
                    .unwrap_or_default();
                let mut flags = String::new();
                if run.synthetic_bold {
                    flags.push_str(" bold");
                }
                if run.synthetic_italic {
                    flags.push_str(" italic");
                }
                if (run.h_scale - 1.0).abs() > 1e-9 {
                    let _ = write!(flags, " scale {}", run.h_scale);
                }
                let words: String = run
                    .clusters
                    .iter()
                    .map(|cluster| cluster.text.as_str())
                    .collect();
                let glyphs: Vec<String> = run
                    .clusters
                    .iter()
                    .flat_map(|cluster| &cluster.glyphs)
                    .map(|glyph| {
                        if twips(glyph.y) == 0 {
                            format!("{}@{}", glyph.glyph.to_u16(), twips(glyph.x))
                        } else {
                            format!(
                                "{}@{},{}",
                                glyph.glyph.to_u16(),
                                twips(glyph.x),
                                twips(glyph.y)
                            )
                        }
                    })
                    .collect();
                writeln!(
                    text,
                    "glyphs {family} {} {} @{},{}{flags} {words:?} {}",
                    twips(run.size),
                    hex(run.color),
                    twips(run.origin.x),
                    twips(run.origin.y),
                    glyphs.join(" ")
                )
            }
            DisplayItem::Rect {
                at,
                width,
                height,
                color,
            } => writeln!(
                text,
                "rect @{},{} {}x{} {}",
                twips(at.x),
                twips(at.y),
                twips(*width),
                twips(*height),
                hex(*color)
            ),
            DisplayItem::Line(line) => {
                let style = match line.style {
                    LineStyle::Solid => "solid",
                    LineStyle::Dotted => "dotted",
                    LineStyle::Dashed => "dashed",
                    LineStyle::LongDashed => "long-dashed",
                    LineStyle::DotDash => "dot-dash",
                    LineStyle::DotDotDash => "dot-dot-dash",
                    LineStyle::Wave => "wave",
                };
                writeln!(
                    text,
                    "line {},{} {},{} {} {style} {}",
                    twips(line.from.x),
                    twips(line.from.y),
                    twips(line.to.x),
                    twips(line.to.y),
                    twips(line.width),
                    hex(line.color)
                )
            }
            DisplayItem::Image(image) => writeln!(
                text,
                "image {} @{},{} {}x{}",
                image.image.index(),
                twips(image.at.x),
                twips(image.at.y),
                twips(image.width),
                twips(image.height)
            ),
        };
    }
    Painted {
        text,
        report: fonts.into_report(),
    }
}
