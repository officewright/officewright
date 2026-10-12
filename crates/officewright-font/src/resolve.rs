use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use crate::book::{FaceId, FaceInfo, FontBook, FontOrigin, name_key};

/// A generic kind of typeface, used to pick a fallback when neither the requested
/// family nor a substitute for it is available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GenericFamily {
    /// Proportional, without serifs, like Arial.
    #[default]
    SansSerif,
    /// Proportional, with serifs, like Times New Roman.
    Serif,
    /// Fixed pitch, like Courier New.
    Monospace,
}

/// The font a document asks for: a family name as the document stores it, and the
/// style of the text.
///
/// ```
/// use officewright_font::{FontRequest, GenericFamily};
///
/// let request = FontRequest::new("Calibri").bold(true).generic(GenericFamily::SansSerif);
/// assert_eq!(request.family(), "Calibri");
/// assert!(request.is_bold());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontRequest {
    family: String,
    bold: bool,
    italic: bool,
    generic: Option<GenericFamily>,
}

impl FontRequest {
    /// A request for the regular style of a family.
    pub fn new(family: impl Into<String>) -> Self {
        Self {
            family: family.into(),
            bold: false,
            italic: false,
            generic: None,
        }
    }

    /// Asks for bold text.
    pub fn bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    /// Asks for italic text.
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Says what kind of typeface the family is, for choosing a fallback. Without it,
    /// the fallback is sans-serif, except for the families whose kind is known: those
    /// with a substitute and the common fonts the fallbacks are chosen from.
    pub fn generic(mut self, generic: GenericFamily) -> Self {
        self.generic = Some(generic);
        self
    }

    /// The requested family.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// Whether bold text is requested.
    pub fn is_bold(&self) -> bool {
        self.bold
    }

    /// Whether italic text is requested.
    pub fn is_italic(&self) -> bool {
        self.italic
    }

    /// The generic family, if one was given.
    pub fn generic_family(&self) -> Option<GenericFamily> {
        self.generic
    }

    /// The request as it is compared: family name without regard to case.
    fn key(&self) -> (String, bool, bool, Option<GenericFamily>) {
        (name_key(&self.family), self.bold, self.italic, self.generic)
    }
}

/// How a [`FontRequest`] was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResolutionKind {
    /// A face has the requested family name (name ID 1, the name Word uses).
    Exact,
    /// Windows maps the requested name to another family that is available, for
    /// example "Times New Roman CYR" to Times New Roman. Word shows the same font, so
    /// this is not a substitution.
    Alias,
    /// No face has the requested family name, but one has it as its typographic family,
    /// full name or PostScript name: for example only "Open Sans SemiBold" is installed
    /// when "Open Sans" is asked for. Word does not find fonts by these names, so the
    /// output may differ.
    OtherName,
    /// The requested name is a named instance of an installed variable font, such as
    /// "Bahnschrift SemiBold". Only the default instance can be used yet, so the weight
    /// or width differs.
    VariableInstance,
    /// The requested family is not available; a font with the same advance widths
    /// replaces it, so text lays out the same.
    MetricCompatible,
    /// Neither the requested family nor a metric-compatible substitute is available; a
    /// font of similar design (Caladea for Cambria) or a common font of the same
    /// generic family replaces it, and text lays out differently.
    Fallback,
    /// No font could be found at all.
    Missing,
}

/// Which name of a face matched, for [`ResolutionKind::OtherName`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OtherName {
    Typographic,
    Full,
    PostScript,
}

/// The face chosen for a [`FontRequest`], and how it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    request: FontRequest,
    face: Option<FaceId>,
    family: Option<String>,
    kind: ResolutionKind,
    other_name: Option<OtherName>,
    instance: Option<String>,
    uninstalled_substitute: Option<String>,
    similar_design: bool,
    synthetic_bold: bool,
    synthetic_italic: bool,
}

impl Resolution {
    /// The request.
    pub fn request(&self) -> &FontRequest {
        &self.request
    }

    /// The chosen face, `None` if there is none ([`ResolutionKind::Missing`]).
    pub fn face(&self) -> Option<FaceId> {
        self.face
    }

    /// The family name of the chosen face.
    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// How the face was chosen.
    pub fn kind(&self) -> ResolutionKind {
        self.kind
    }

    /// For [`ResolutionKind::VariableInstance`], the subfamily name of the requested
    /// instance, such as "SemiBold".
    pub fn variable_instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// For [`ResolutionKind::Fallback`], the metric-compatible substitute that would
    /// have been used had it been installed, if the family has one.
    pub fn uninstalled_substitute(&self) -> Option<&str> {
        self.uninstalled_substitute.as_deref()
    }

    /// Whether bold was requested but the face is not bold, so it has to be emboldened
    /// artificially, as Word does.
    pub fn synthetic_bold(&self) -> bool {
        self.synthetic_bold
    }

    /// Whether italic was requested but the face is upright, so it has to be slanted
    /// artificially, as Word does.
    pub fn synthetic_italic(&self) -> bool {
        self.synthetic_italic
    }

    /// Whether the output will differ from the document as its author saw it: another
    /// family, another instance, or a synthesized style.
    pub fn is_substitution(&self) -> bool {
        !matches!(self.kind, ResolutionKind::Exact | ResolutionKind::Alias)
            || self.synthetic_bold
            || self.synthetic_italic
    }

    /// Whether two resolutions report the same thing.
    fn same_outcome(&self, other: &Self) -> bool {
        name_key(&self.request.family) == name_key(&other.request.family)
            && self.request.bold == other.request.bold
            && self.request.italic == other.request.italic
            && self.face == other.face
            && self.kind == other.kind
            && self.synthetic_bold == other.synthetic_bold
            && self.synthetic_italic == other.synthetic_italic
    }
}

impl fmt::Display for Resolution {
    /// One line for a report, for example `"Calibri" bold → "Carlito" (metric-compatible
    /// substitute)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.request.family)?;
        match (self.request.bold, self.request.italic) {
            (true, true) => f.write_str(" bold italic")?,
            (true, false) => f.write_str(" bold")?,
            (false, true) => f.write_str(" italic")?,
            (false, false) => {}
        }
        let Some(family) = &self.family else {
            return f.write_str(" → no font available");
        };
        write!(f, " → \"{family}\"")?;
        match self.kind {
            ResolutionKind::Exact | ResolutionKind::Missing => {}
            ResolutionKind::Alias => f.write_str(" (Windows font alias)")?,
            ResolutionKind::OtherName => {
                let name = match self.other_name {
                    Some(OtherName::Typographic) => "typographic family name",
                    Some(OtherName::Full) => "full name",
                    Some(OtherName::PostScript) | None => "PostScript name",
                };
                write!(f, " (no family of that name; matched by {name})")?;
            }
            ResolutionKind::VariableInstance => write!(
                f,
                " (named instance \"{}\" of a variable font; its default instance is used)",
                self.instance.as_deref().unwrap_or_default()
            )?,
            ResolutionKind::MetricCompatible => f.write_str(" (metric-compatible substitute)")?,
            ResolutionKind::Fallback if self.similar_design => f.write_str(
                " (fallback of similar design; it is not metric-compatible, so text may lay \
                 out differently than in Word)",
            )?,
            ResolutionKind::Fallback => match &self.uninstalled_substitute {
                Some(substitute) => write!(
                    f,
                    " (fallback; metric-compatible substitute \"{substitute}\" is not installed)"
                )?,
                None => f.write_str(" (fallback; not installed)")?,
            },
        }
        match (self.synthetic_bold, self.synthetic_italic) {
            (true, true) => f.write_str(" with synthetic bold and italic"),
            (true, false) => f.write_str(" with synthetic bold"),
            (false, true) => f.write_str(" with synthetic italic"),
            (false, false) => Ok(()),
        }
    }
}

/// Font names Windows maps to other families: the `FontSubstitutes` key of the
/// registry (`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontSubstitutes`) as
/// Windows 10.0.26300 has it. Word resolves these names without telling, so a document
/// that an old Word version wrote with "Times New Roman CYR" shows Times New Roman.
/// The character sets of the registry entries (",204") are left out, since text is
/// Unicode, and so is the one entry that names a style ("Arabic Transparent Bold").
const WINDOWS_ALIASES: [(&str, &str); 23] = [
    ("arabic transparent", "Arial"),
    ("arial baltic", "Arial"),
    ("arial ce", "Arial"),
    ("arial cyr", "Arial"),
    ("arial greek", "Arial"),
    ("arial tur", "Arial"),
    ("courier new baltic", "Courier New"),
    ("courier new ce", "Courier New"),
    ("courier new cyr", "Courier New"),
    ("courier new greek", "Courier New"),
    ("courier new tur", "Courier New"),
    ("helv", "MS Sans Serif"),
    ("helvetica", "Arial"),
    ("ms shell dlg", "Microsoft Sans Serif"),
    ("ms shell dlg 2", "Tahoma"),
    ("tahoma armenian", "Tahoma"),
    ("times", "Times New Roman"),
    ("times new roman baltic", "Times New Roman"),
    ("times new roman ce", "Times New Roman"),
    ("times new roman cyr", "Times New Roman"),
    ("times new roman greek", "Times New Roman"),
    ("times new roman tur", "Times New Roman"),
    ("tms rmn", "MS Serif"),
];

/// Families of Microsoft fonts and free fonts with the same advance widths, so a
/// document lays out the same with either.
///
/// Sources: the Carlito README ("metric-compatible with Calibri",
/// <https://github.com/googlefonts/carlito>), the Liberation README ("metric
/// compatibility as usage of Times New Roman, Arial, Courier New",
/// <https://github.com/liberationfonts/liberation-fonts>), and fontconfig's
/// `conf.d/30-metric-aliases.conf`. The generic family is the fallback when neither
/// font is installed and the request does not give one. `tests/metric_compatible.rs`
/// measures each pair against Word's reference render of `E-line-fonts`.
///
/// fontconfig also pairs Cambria with Caladea, wrongly for the Cambria Word uses. The
/// Word reference renders in `tests/corpus/reference/word-rules/E-line-fonts`
/// (`geometry.json`) show Caladea's advance widths differing from Cambria's by up to
/// 0.19 em (Cambria's digits are wider), against at most 0.003 em between Carlito and
/// Calibri, and a line of 11 pt text 14.30 pt high with Caladea's metrics where Word
/// sets Cambria's 12.90 pt apart. Caladea is in [`SIMILAR_DESIGNS`] instead.
const METRIC_COMPATIBLE: [(&str, &str, GenericFamily); 4] = [
    ("calibri", "Carlito", GenericFamily::SansSerif),
    ("arial", "Liberation Sans", GenericFamily::SansSerif),
    ("times new roman", "Liberation Serif", GenericFamily::Serif),
    ("courier new", "Liberation Mono", GenericFamily::Monospace),
];

/// Families of Microsoft fonts and free fonts of similar design that are not
/// metric-compatible: the free font is the closest stand-in, but text lays out
/// differently, so it is reported as a fallback. See [`METRIC_COMPATIBLE`] for why
/// Caladea is here.
const SIMILAR_DESIGNS: [(&str, &str, GenericFamily); 1] =
    [("cambria", "Caladea", GenericFamily::Serif)];

/// Fallback families for each generic family, in order of preference: the Microsoft
/// font, its metric-compatible substitutes, then fonts common on macOS and Linux. A
/// requested family that is in one of these lists has that generic family.
///
/// Word itself falls back through the Windows font mapper, which uses the PANOSE,
/// character set and pitch that the document's font table records; this list is an
/// approximation until the document readers pass those on.
const FALLBACKS: [(GenericFamily, [&str; 6]); 3] = [
    (
        GenericFamily::SansSerif,
        [
            "Arial",
            "Liberation Sans",
            "Arimo",
            "Helvetica",
            "DejaVu Sans",
            "Noto Sans",
        ],
    ),
    (
        GenericFamily::Serif,
        [
            "Times New Roman",
            "Liberation Serif",
            "Tinos",
            "Times",
            "DejaVu Serif",
            "Noto Serif",
        ],
    ),
    (
        GenericFamily::Monospace,
        [
            "Courier New",
            "Liberation Mono",
            "Cousine",
            "Courier",
            "DejaVu Sans Mono",
            "Noto Sans Mono",
        ],
    ),
];

/// The weight from which a face counts as bold. Word knows only bold on or off; a
/// lighter face is emboldened artificially. 600 is `FW_SEMIBOLD` among the weights of
/// Windows' LOGFONT. The threshold is ours: Microsoft does not document when Word or
/// Windows embolden, so it is to be checked against Word reference renders once the
/// corpus has them (Milestone 4).
const BOLD_WEIGHT: u16 = 600;

impl FontBook {
    /// Finds the face for a request, without caching or reporting; [`FontResolver`]
    /// does both.
    ///
    /// In order, the first that is installed wins:
    ///
    /// 1. A face whose family name (name ID 1, in any language) is the requested one,
    ///    compared without regard to case. This is how Word names fonts: "Calibri
    ///    Light" is a family of its own.
    /// 2. The family Windows maps the name to, such as Times New Roman for "Times" or
    ///    "Times New Roman CYR".
    /// 3. A face whose typographic family name (16), full name (4) or PostScript name
    ///    (6) is the requested one.
    /// 4. A named instance of a variable font, such as "Bahnschrift SemiBold". Only the
    ///    default instance of a variable font is used yet; the resolution says so.
    /// 5. The metric-compatible substitute: Carlito for Calibri, Liberation Sans, Serif
    ///    and Mono for Arial, Times New Roman and Courier New.
    /// 6. A font of similar design that lays out differently: Caladea for Cambria. It
    ///    is reported as a fallback.
    /// 7. A common font of the request's generic family, or of the family's own if it
    ///    is a known one, else sans-serif.
    ///
    /// Among the faces of the family, the one closest to the requested style wins;
    /// ties go to document fonts, then user fonts, then system fonts.
    pub fn resolve(&self, request: &FontRequest) -> Resolution {
        let resolution = |face: Option<(FaceId, &FaceInfo)>, kind| {
            let family = face.and_then(|(_, info)| info.names().family().map(str::to_owned));
            Resolution {
                request: request.clone(),
                face: face.map(|(id, _)| id),
                family,
                kind,
                other_name: None,
                instance: None,
                uninstalled_substitute: None,
                similar_design: false,
                synthetic_bold: face
                    .is_some_and(|(_, info)| request.bold && info.weight() < BOLD_WEIGHT),
                synthetic_italic: face.is_some_and(|(_, info)| request.italic && !info.is_italic()),
            }
        };
        let key = name_key(&request.family);
        if key.is_empty() {
            return resolution(None, ResolutionKind::Missing);
        }
        if let Some(face) = self.find_family(&key, request) {
            return resolution(Some(face), ResolutionKind::Exact);
        }
        let alias = WINDOWS_ALIASES
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, target)| *target);
        if let Some(face) = alias.and_then(|target| self.find_family(&name_key(target), request)) {
            return resolution(Some(face), ResolutionKind::Alias);
        }
        if let Some((face, other_name)) = self.find_other_name(&key, request) {
            return Resolution {
                other_name: Some(other_name),
                ..resolution(Some(face), ResolutionKind::OtherName)
            };
        }
        if let Some((face, instance)) = self.find_instance(&key, request) {
            return Resolution {
                instance: Some(instance.to_owned()),
                ..resolution(Some(face), ResolutionKind::VariableInstance)
            };
        }
        // The family the name stands for: the alias target, or the name itself.
        let family = alias.map_or(key, name_key);
        let known = METRIC_COMPATIBLE
            .iter()
            .find(|(name, _, _)| *name == family);
        let substitute = known.map(|(_, substitute, _)| *substitute);
        if let Some(face) = substitute.and_then(|substitute| self.find_any(substitute, request)) {
            return resolution(Some(face), ResolutionKind::MetricCompatible);
        }
        let similar_design = SIMILAR_DESIGNS.iter().find(|(name, _, _)| *name == family);
        if let Some(face) = similar_design.and_then(|(_, name, _)| self.find_any(name, request)) {
            return Resolution {
                similar_design: true,
                ..resolution(Some(face), ResolutionKind::Fallback)
            };
        }
        let generic = request
            .generic
            .or(known.or(similar_design).map(|(_, _, generic)| *generic))
            .or_else(|| {
                FALLBACKS
                    .iter()
                    .find(|(_, names)| names.iter().any(|name| name_key(name) == family))
                    .map(|(generic, _)| *generic)
            })
            .unwrap_or_default();
        let fallbacks = FALLBACKS
            .iter()
            .find(|(candidate, _)| *candidate == generic)
            .map(|(_, names)| names.as_slice())
            .unwrap_or_default();
        for name in fallbacks {
            if let Some(face) = self.find_any(name, request) {
                return Resolution {
                    uninstalled_substitute: substitute.map(str::to_owned),
                    ..resolution(Some(face), ResolutionKind::Fallback)
                };
            }
        }
        resolution(None, ResolutionKind::Missing)
    }

    /// The face of the family with the normalized name `key` that best fits the style
    /// of `request`.
    fn find_family(&self, key: &str, request: &FontRequest) -> Option<(FaceId, &FaceInfo)> {
        self.best(request, |face| {
            let found = face.keys().families.iter().any(|name| name == key);
            found.then_some((face.weight(), face.is_italic(), ()))
        })
        .map(|(face, ())| face)
    }

    /// The best face with `key` as its typographic family, full or PostScript name.
    fn find_other_name(
        &self,
        key: &str,
        request: &FontRequest,
    ) -> Option<((FaceId, &FaceInfo), OtherName)> {
        type Matches = fn(&FaceInfo, &str) -> bool;
        let tiers: [(OtherName, Matches); 3] = [
            (OtherName::Typographic, |face, key| {
                face.keys()
                    .typographic_families
                    .iter()
                    .any(|name| name == key)
            }),
            (OtherName::Full, |face, key| {
                face.keys().full_names.iter().any(|name| name == key)
            }),
            (OtherName::PostScript, |face, key| {
                face.keys().postscript_name.as_deref() == Some(key)
            }),
        ];
        tiers.iter().find_map(|&(other_name, matches)| {
            self.best(request, |face| {
                matches(face, key).then_some((face.weight(), face.is_italic(), other_name))
            })
        })
    }

    /// The best face with a named instance called `key`: one of the face's family names
    /// followed by the instance's words other than Regular, Bold and Italic, in any
    /// order. Returns the instance's subfamily name too.
    fn find_instance(
        &self,
        key: &str,
        request: &FontRequest,
    ) -> Option<((FaceId, &FaceInfo), &str)> {
        self.best(request, |face| {
            let keys = face.keys();
            let families = keys.families.iter().chain(&keys.typographic_families);
            let instance = families.into_iter().find_map(|family| {
                let rest = key.strip_prefix(family.as_str())?.strip_prefix(' ')?;
                let mut words: Vec<&str> = rest.split_whitespace().collect();
                words.sort_unstable();
                keys.instances
                    .iter()
                    .find(|instance| instance.words == words)
            })?;
            Some((instance.weight, instance.italic, instance.name.as_str()))
        })
    }

    /// Among the faces that `style` matches, giving each a weight, a slant and a value
    /// to return, the closest to the requested style; ties go to the origin, then to
    /// the face added first.
    fn best<'a, T>(
        &'a self,
        request: &FontRequest,
        style: impl Fn(&'a FaceInfo) -> Option<(u16, bool, T)>,
    ) -> Option<((FaceId, &'a FaceInfo), T)> {
        self.faces()
            .filter_map(|(id, face)| style(face).map(|style| (id, face, style)))
            .min_by_key(|(id, face, (weight, italic, _))| {
                (
                    style_distance(*weight, *italic, request),
                    origin_rank(face.origin()),
                    *id,
                )
            })
            .map(|(id, face, (_, _, value))| ((id, face), value))
    }

    /// The best face of `family` by any of its names.
    fn find_any(&self, family: &str, request: &FontRequest) -> Option<(FaceId, &FaceInfo)> {
        let key = name_key(family);
        self.find_family(&key, request)
            .or_else(|| self.find_other_name(&key, request).map(|(face, _)| face))
    }
}

/// How far a face's style is from the requested one: a slant mismatch first (a slant
/// can be synthesized, but an upright request should not get an italic face either),
/// then the weight.
fn style_distance(weight: u16, italic: bool, request: &FontRequest) -> (bool, u16) {
    let target: u16 = if request.bold { 700 } else { 400 };
    (italic != request.italic, weight.abs_diff(target))
}

fn origin_rank(origin: FontOrigin) -> u8 {
    match origin {
        FontOrigin::Document => 0,
        FontOrigin::User => 1,
        FontOrigin::System => 2,
    }
}

/// Resolves the font requests of a document against a [`FontBook`], remembers the
/// answers and keeps the report of substitutions.
///
/// ```
/// use officewright_font::{FontBook, FontRequest, FontResolver, ResolutionKind};
///
/// let book = FontBook::new();
/// let mut resolver = FontResolver::new(&book);
/// assert_eq!(resolver.resolve(&FontRequest::new("Calibri")).kind(), ResolutionKind::Missing);
/// assert_eq!(resolver.substitutions().len(), 1);
/// assert_eq!(
///     resolver.substitutions()[0].to_string(),
///     "\"Calibri\" → no font available"
/// );
/// ```
///
/// A resolver borrows its book; [`from_shared`](Self::from_shared) makes one that
/// shares it instead, for a resolver that must outlive the borrow.
#[derive(Debug)]
pub struct FontResolver<'a> {
    book: BookRef<'a>,
    cache: HashMap<(String, bool, bool, Option<GenericFamily>), Resolution>,
    substitutions: Vec<Resolution>,
}

/// The book of a [`FontResolver`], borrowed or shared.
#[derive(Debug)]
enum BookRef<'a> {
    Borrowed(&'a FontBook),
    Shared(Arc<FontBook>),
}

impl Deref for BookRef<'_> {
    type Target = FontBook;

    fn deref(&self) -> &FontBook {
        match self {
            Self::Borrowed(book) => book,
            Self::Shared(book) => book,
        }
    }
}

impl FontResolver<'static> {
    /// A resolver with an empty report that shares `book` instead of borrowing it, so
    /// it can be kept as long as needed, for example in a handle of the C ABI.
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use officewright_font::{FontBook, FontRequest, FontResolver, ResolutionKind};
    ///
    /// let mut book = FontBook::new();
    /// book.add_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"))?;
    /// let mut resolver = FontResolver::from_shared(Arc::new(book));
    /// let resolution = resolver.resolve(&FontRequest::new("Officewright Test"));
    /// assert_eq!(resolution.kind(), ResolutionKind::Exact);
    /// # Ok::<(), officewright_font::Error>(())
    /// ```
    pub fn from_shared(book: Arc<FontBook>) -> Self {
        Self::with(BookRef::Shared(book))
    }
}

impl<'a> FontResolver<'a> {
    /// A resolver with an empty report.
    pub fn new(book: &'a FontBook) -> Self {
        Self::with(BookRef::Borrowed(book))
    }

    fn with(book: BookRef<'a>) -> Self {
        Self {
            book,
            cache: HashMap::new(),
            substitutions: Vec::new(),
        }
    }

    /// The book the resolver resolves against, to [`load`](FontBook::load) the faces it
    /// chooses.
    pub fn book(&self) -> &FontBook {
        &self.book
    }

    /// Resolves a request like [`FontBook::resolve`]. A request that leads to a
    /// substitution is added to the report the first time it is resolved.
    pub fn resolve(&mut self, request: &FontRequest) -> &Resolution {
        let book = &*self.book;
        let substitutions = &mut self.substitutions;
        self.cache.entry(request.key()).or_insert_with(|| {
            let resolution = book.resolve(request);
            let reported = substitutions
                .iter()
                .any(|known| known.same_outcome(&resolution));
            if resolution.is_substitution() && !reported {
                substitutions.push(resolution.clone());
            }
            resolution
        })
    }

    /// Every request so far whose output differs from the document: another family,
    /// another instance or a synthesized style, each outcome once, in the order they
    /// were first resolved.
    pub fn substitutions(&self) -> &[Resolution] {
        &self.substitutions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestFont;

    fn book(fonts: &[(TestFont, FontOrigin)]) -> FontBook {
        let mut book = FontBook::new();
        for (font, origin) in fonts {
            book.add_bytes(font.build(), *origin).unwrap();
        }
        book
    }

    fn system(font: TestFont) -> (TestFont, FontOrigin) {
        (font, FontOrigin::System)
    }

    fn resolved<'a>(book: &'a FontBook, request: &FontRequest) -> (&'a FaceInfo, Resolution) {
        let resolution = book.resolve(request);
        let face = book.face(resolution.face().unwrap()).unwrap();
        (face, resolution)
    }

    #[test]
    fn finds_a_family_without_regard_to_case() {
        let book = book(&[system(TestFont::new("Test Sans"))]);
        let (face, resolution) = resolved(&book, &FontRequest::new(" test SANS "));
        assert_eq!(face.names().family(), Some("Test Sans"));
        assert_eq!(resolution.kind(), ResolutionKind::Exact);
        assert!(!resolution.is_substitution());
    }

    #[test]
    fn legacy_family_names_are_families_of_their_own() {
        // As in Office: "Calibri Light" is its own family, typographically "Calibri".
        let book = book(&[
            system(
                TestFont::new("Test Sans Light")
                    .typographic("Test Sans", "Light")
                    .style("Regular", 300, false),
            ),
            system(TestFont::new("Test Sans")),
            system(TestFont::new("Test Sans").style("Bold", 700, false)),
        ]);
        let (face, _) = resolved(&book, &FontRequest::new("Test Sans Light"));
        assert_eq!(face.weight(), 300);
        let (face, _) = resolved(&book, &FontRequest::new("Test Sans"));
        assert_eq!(face.weight(), 400);
        let (face, resolution) = resolved(&book, &FontRequest::new("Test Sans").bold(true));
        assert_eq!(face.weight(), 700);
        assert!(!resolution.synthetic_bold());
    }

    #[test]
    fn other_names_match_but_are_reported() {
        // Only "Test Sans Semibold" is installed: Word would not find "Test Sans".
        let book = book(&[
            system(
                TestFont::new("Test Sans Semibold")
                    .typographic("Test Sans", "Semibold")
                    .style("Regular", 600, false),
            ),
            system(TestFont::new("Test Serif").style("Bold", 700, false)),
        ]);
        let (face, resolution) = resolved(&book, &FontRequest::new("Test Sans"));
        assert_eq!(face.weight(), 600);
        assert_eq!(resolution.kind(), ResolutionKind::OtherName);
        assert!(resolution.is_substitution());
        assert_eq!(
            resolution.to_string(),
            "\"Test Sans\" → \"Test Sans Semibold\" (no family of that name; matched by \
             typographic family name)"
        );
        let (face, resolution) = resolved(&book, &FontRequest::new("Test Serif Bold"));
        assert_eq!(face.names().family(), Some("Test Serif"));
        assert!(resolution.to_string().ends_with("matched by full name)"));
        let (_, resolution) = resolved(&book, &FontRequest::new("TestSansSemibold-Regular"));
        assert_eq!(resolution.kind(), ResolutionKind::OtherName);
        assert!(
            resolution
                .to_string()
                .ends_with("matched by PostScript name)")
        );
    }

    #[test]
    fn windows_aliases_are_the_same_font() {
        let book = book(&[
            system(TestFont::new("Times New Roman")),
            system(TestFont::new("Liberation Sans")),
            system(TestFont::new("Liberation Mono")),
        ]);
        for name in ["Times", "Times New Roman CYR", "times new roman ce"] {
            let resolution = book.resolve(&FontRequest::new(name));
            assert_eq!(resolution.kind(), ResolutionKind::Alias, "{name}");
            assert_eq!(resolution.family(), Some("Times New Roman"), "{name}");
            assert!(!resolution.is_substitution(), "{name}");
        }
        // Without the alias target, its metric-compatible substitute.
        let resolution = book.resolve(&FontRequest::new("Arial CYR"));
        assert_eq!(resolution.kind(), ResolutionKind::MetricCompatible);
        assert_eq!(resolution.family(), Some("Liberation Sans"));
        // A known family name gives its generic family to the fallback.
        let resolution = book.resolve(&FontRequest::new("Courier"));
        assert_eq!(resolution.kind(), ResolutionKind::Fallback);
        assert_eq!(resolution.family(), Some("Liberation Mono"));
    }

    #[test]
    fn named_instances_of_variable_fonts_are_recognized() {
        let axes = [
            (b"wght", 300.0, 400.0, 700.0),
            (b"wdth", 75.0, 100.0, 100.0),
        ];
        let instances: [(u16, &[f64]); 4] = [
            (256, &[400.0, 100.0]),
            (257, &[600.0, 100.0]),
            (258, &[700.0, 100.0]),
            (259, &[300.0, 75.0]),
        ];
        let variable = TestFont::new("Test Var")
            .name_record(256, "Regular")
            .name_record(257, "SemiBold")
            .name_record(258, "Bold")
            .name_record(259, "Light Condensed")
            .with_table(b"fvar", crate::testing::fvar(&axes, &instances));
        let book = book(&[system(variable)]);
        let resolution = book.resolve(&FontRequest::new("Test Var SemiBold"));
        assert_eq!(resolution.kind(), ResolutionKind::VariableInstance);
        assert_eq!(resolution.variable_instance(), Some("SemiBold"));
        assert!(resolution.is_substitution());
        assert_eq!(
            resolution.to_string(),
            "\"Test Var SemiBold\" → \"Test Var\" (named instance \"SemiBold\" of a variable \
             font; its default instance is used)"
        );
        // Word's names put the words in either order.
        for name in ["Test Var Light Condensed", "test var condensed light"] {
            let resolution = book.resolve(&FontRequest::new(name));
            assert_eq!(
                resolution.variable_instance(),
                Some("Light Condensed"),
                "{name}"
            );
        }
        // Style words belong to the style, not to an instance name.
        assert_ne!(
            book.resolve(&FontRequest::new("Test Var Bold")).kind(),
            ResolutionKind::VariableInstance
        );
        assert_eq!(
            book.resolve(&FontRequest::new("Test Var")).kind(),
            ResolutionKind::Exact
        );
    }

    #[test]
    fn localized_family_names_match() {
        let book = book(&[system(
            TestFont::new("SimTest").localized_family(0x0804, "测试宋体"),
        )]);
        let (face, _) = resolved(&book, &FontRequest::new("测试宋体"));
        assert_eq!(face.names().family(), Some("SimTest"));
    }

    #[test]
    fn synthesizes_missing_styles() {
        let book = book(&[system(TestFont::new("Test Sans"))]);
        let resolution = book.resolve(&FontRequest::new("Test Sans").bold(true).italic(true));
        assert_eq!(resolution.kind(), ResolutionKind::Exact);
        assert!(resolution.synthetic_bold());
        assert!(resolution.synthetic_italic());
        assert!(resolution.is_substitution());
        assert_eq!(
            resolution.to_string(),
            "\"Test Sans\" bold italic → \"Test Sans\" with synthetic bold and italic"
        );
        let resolution = book.resolve(&FontRequest::new("Test Sans").italic(true));
        assert!(resolution.synthetic_italic());
        assert!(!resolution.synthetic_bold());
        assert!(resolution.is_substitution());
    }

    #[test]
    fn faces_from_600_count_as_bold() {
        let book = book(&[
            system(TestFont::new("Test Semi").style("SemiBold", 600, false)),
            system(TestFont::new("Test Medium").style("Medium", 500, false)),
        ]);
        let request = |family| FontRequest::new(family).bold(true);
        assert!(!book.resolve(&request("Test Semi")).synthetic_bold());
        assert!(book.resolve(&request("Test Medium")).synthetic_bold());
    }

    #[test]
    fn regular_text_takes_the_weight_closest_to_400() {
        let book = book(&[
            system(TestFont::new("Test Sans").style("Light", 300, false)),
            system(TestFont::new("Test Sans")),
            system(TestFont::new("Test Sans").style("Medium", 500, false)),
        ]);
        let (face, _) = resolved(&book, &FontRequest::new("Test Sans"));
        assert_eq!(face.weight(), 400);
    }

    #[test]
    fn upright_text_prefers_upright_faces() {
        let book = book(&[
            system(TestFont::new("Test Sans").style("Italic", 400, true)),
            system(TestFont::new("Test Sans").style("Bold", 700, false)),
        ]);
        let (face, resolution) = resolved(&book, &FontRequest::new("Test Sans"));
        assert!(!face.is_italic());
        assert_eq!(face.weight(), 700);
        assert!(!resolution.synthetic_italic());
    }

    #[test]
    fn document_fonts_win_ties_but_not_over_a_better_style() {
        let book = book(&[
            system(TestFont::new("Test Sans")),
            system(TestFont::new("Test Sans").style("Bold", 700, false)),
            (TestFont::new("Test Sans"), FontOrigin::Document),
        ]);
        let (face, _) = resolved(&book, &FontRequest::new("Test Sans"));
        assert_eq!(face.origin(), FontOrigin::Document);
        let (face, _) = resolved(&book, &FontRequest::new("Test Sans").bold(true));
        assert_eq!(face.origin(), FontOrigin::System);
        assert_eq!(face.weight(), 700);
    }

    #[test]
    fn substitutes_metric_compatible_fonts() {
        let book = book(&[
            system(TestFont::new("Carlito")),
            system(TestFont::new("Carlito").style("Bold", 700, false)),
            system(TestFont::new("Liberation Sans")),
        ]);
        let resolution = book.resolve(&FontRequest::new("Calibri").bold(true));
        assert_eq!(resolution.kind(), ResolutionKind::MetricCompatible);
        assert_eq!(resolution.family(), Some("Carlito"));
        assert!(!resolution.synthetic_bold());
        assert_eq!(
            resolution.to_string(),
            "\"Calibri\" bold → \"Carlito\" (metric-compatible substitute)"
        );
    }

    #[test]
    fn every_metric_compatible_pair_is_measured_against_word() {
        // tests/metric_compatible.rs measures these pairs: a new one needs its Microsoft
        // font in a Word reference render and its free font in tests/fonts there first.
        let measured = ["calibri", "arial", "times new roman", "courier new"];
        assert_eq!(METRIC_COMPATIBLE.map(|(family, _, _)| family), measured);
    }

    #[test]
    fn falls_back_when_the_substitute_is_not_installed() {
        let book = book(&[
            system(TestFont::new("Liberation Sans")),
            system(TestFont::new("Liberation Serif")),
        ]);
        let resolution = book.resolve(&FontRequest::new("Calibri"));
        assert_eq!(resolution.family(), Some("Liberation Sans"));
        assert_eq!(resolution.kind(), ResolutionKind::Fallback);
        assert_eq!(resolution.uninstalled_substitute(), Some("Carlito"));
        assert_eq!(
            resolution.to_string(),
            "\"Calibri\" → \"Liberation Sans\" (fallback; metric-compatible substitute \
             \"Carlito\" is not installed)"
        );
        let resolution = book.resolve(&FontRequest::new("Cambria"));
        assert_eq!(
            resolution.family(),
            Some("Liberation Serif"),
            "Cambria is a serif"
        );
        assert_eq!(resolution.uninstalled_substitute(), None);
        assert_eq!(
            resolution.to_string(),
            "\"Cambria\" → \"Liberation Serif\" (fallback; not installed)"
        );
        let resolution = book.resolve(&FontRequest::new("Garamond").generic(GenericFamily::Serif));
        assert_eq!(resolution.family(), Some("Liberation Serif"));
        assert_eq!(
            resolution.to_string(),
            "\"Garamond\" → \"Liberation Serif\" (fallback; not installed)"
        );
    }

    #[test]
    fn a_font_of_similar_design_is_a_fallback() {
        // Caladea looks like Cambria but does not lay out like it: see METRIC_COMPATIBLE.
        let book = book(&[
            system(TestFont::new("Liberation Serif")),
            system(TestFont::new("Caladea")),
            system(TestFont::new("Caladea").style("Bold", 700, false)),
        ]);
        let resolution = book.resolve(&FontRequest::new("Cambria").bold(true));
        assert_eq!(resolution.family(), Some("Caladea"));
        assert_eq!(resolution.kind(), ResolutionKind::Fallback);
        assert!(!resolution.synthetic_bold());
        assert!(resolution.is_substitution());
        assert_eq!(resolution.uninstalled_substitute(), None);
        assert_eq!(
            resolution.to_string(),
            "\"Cambria\" bold → \"Caladea\" (fallback of similar design; it is not \
             metric-compatible, so text may lay out differently than in Word)"
        );
        // The generic family of the request does not outrank the closer design.
        let resolution =
            book.resolve(&FontRequest::new("cambria").generic(GenericFamily::SansSerif));
        assert_eq!(resolution.family(), Some("Caladea"));
    }

    #[test]
    fn missing_when_nothing_fits() {
        let book = book(&[system(TestFont::new("Test Sans"))]);
        let resolution = book.resolve(&FontRequest::new("Garamond"));
        assert_eq!(resolution.kind(), ResolutionKind::Missing);
        assert_eq!(resolution.face(), None);
        assert_eq!(resolution.to_string(), "\"Garamond\" → no font available");
        assert_eq!(
            book.resolve(&FontRequest::new("")).kind(),
            ResolutionKind::Missing
        );
    }

    #[test]
    fn a_shared_resolver_answers_like_a_borrowing_one() {
        let book = book(&[
            system(TestFont::new("Liberation Sans")),
            system(TestFont::new("Test Sans")),
        ]);
        let requests = [
            FontRequest::new("Arial"),
            FontRequest::new("Test Sans").bold(true),
            FontRequest::new("Calibri"),
        ];
        let mut borrowing = FontResolver::new(&book);
        let mut shared = FontResolver::from_shared(Arc::new(book.clone()));
        for request in &requests {
            assert_eq!(borrowing.resolve(request), shared.resolve(request));
        }
        assert_eq!(borrowing.substitutions(), shared.substitutions());
        assert_eq!(shared.book().len(), 2);
    }

    #[test]
    fn the_report_lists_each_substitution_once() {
        let book = book(&[
            system(TestFont::new("Liberation Sans")),
            system(TestFont::new("Test Sans")),
        ]);
        let mut resolver = FontResolver::new(&book);
        for _ in 0..2 {
            resolver.resolve(&FontRequest::new("Arial"));
            resolver.resolve(&FontRequest::new("ARIAL"));
            // A generic family hint that changes nothing does not repeat the line.
            resolver.resolve(&FontRequest::new("Arial").generic(GenericFamily::SansSerif));
            resolver.resolve(&FontRequest::new("Test Sans"));
            resolver.resolve(&FontRequest::new("Test Sans").bold(true));
        }
        let report: Vec<String> = resolver
            .substitutions()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            report,
            [
                "\"Arial\" → \"Liberation Sans\" (metric-compatible substitute)",
                "\"Test Sans\" bold → \"Test Sans\" with synthetic bold",
            ]
        );
    }
}
