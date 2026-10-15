use std::path::Path;
use std::sync::Arc;
use std::{fmt, fs};

use officewright_core::Pt;
use officewright_font::{
    CharMap, EmbeddingLicense, FontFace, FontRequest, FontResolver, OutlineFormat,
};

use crate::{Error, Result, embed, encoding, helvetica, script};

/// A font to draw and measure text with.
///
/// - [`Font::standard`] gives Helvetica, one of the fonts every PDF viewer provides, so
///   nothing is embedded. It is used with WinAnsiEncoding, which covers Latin text only.
/// - [`Font::from_file`], [`Font::from_bytes`] and [`Font::from_face`] give a TrueType or
///   OpenType font that is embedded in the documents it is drawn in and, unless its
///   license forbids subsetting, reduced to the glyphs they use. It shows every
///   character it has a glyph for in the scripts that need no shaping;
///   [`Page::add_text`](crate::Page::add_text) lists them.
///
/// Cloning is cheap: the font data is shared. Two fonts are equal if they are the same
/// standard font or the same face of the same font data, and a document embeds each
/// font once however many pages use it. A `Font` is `Send` and `Sync`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Standard(StandardFont),
    Embedded(Arc<Embedded>),
}

impl PartialEq for Kind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Standard(a), Self::Standard(b)) => a == b,
            (Self::Embedded(a), Self::Embedded(b)) => {
                Arc::ptr_eq(a, b)
                    || (a.face.index() == b.face.index() && a.face.data() == b.face.data())
            }
            _ => false,
        }
    }
}

impl Eq for Kind {}

/// A TrueType or OpenType face that may be embedded.
#[derive(Debug)]
pub(crate) struct Embedded {
    pub(crate) face: FontFace,
    /// The PostScript name, or a name made from the family names if the face has none,
    /// cut to what the PDF's names can hold.
    pub(crate) name: String,
    /// The license forbids subsetting, so every glyph is embedded.
    pub(crate) whole: bool,
    notes: Vec<EmbeddingNote>,
}

/// The standard fonts that PDF viewers provide without embedding (ISO 32000-1, 9.6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StandardFont {
    /// Helvetica, with WinAnsiEncoding.
    Helvetica,
}

/// Something about an embedded font that its user should know, from its license or its
/// format; none of these stops the font from being used. [`Font::embedding_notes`] lists
/// them, for a report next to the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EmbeddingNote {
    /// The license lets the embedded font be used only to view and print the document,
    /// not to edit it ("Preview & Print" embedding, bit 2 of `fsType` in the `OS/2`
    /// table).
    PreviewAndPrint,
    /// The license lets the embedded font be used to view, print and edit the document,
    /// but not installed ("Editable" embedding, bit 3 of `fsType`).
    Editable,
    /// The license forbids subsetting (bit 8 of `fsType`), so every glyph of the font is
    /// embedded, not only the ones the document uses, and the file is larger.
    WholeFont,
    /// The font is a variable font; only its default instance is embedded.
    DefaultInstance,
}

impl fmt::Display for EmbeddingNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PreviewAndPrint => {
                "its license allows the embedded font only for viewing and printing the document"
            }
            Self::Editable => {
                "its license allows the embedded font for viewing, printing and editing the \
                 document, not for installing it"
            }
            Self::WholeFont => "its license forbids subsetting, so the whole font is embedded",
            Self::DefaultInstance => "it is a variable font; only its default instance is embedded",
        })
    }
}

impl Font {
    /// The smallest font size accepted. Smaller text is invisible, and its size would
    /// lose its digits in the four decimals the file stores.
    pub const MIN_SIZE: Pt = Pt(0.01);

    /// The largest font size accepted: 32767, the largest real number every PDF 1.4
    /// reader has to handle (PDF Reference 1.4, Appendix C).
    pub const MAX_SIZE: Pt = Pt(32767.0);

    /// A standard font.
    pub fn standard(font: StandardFont) -> Self {
        Self {
            kind: Kind::Standard(font),
        }
    }

    /// Reads face `face_index` of a TrueType or OpenType font file or collection (`.ttf`,
    /// `.otf`, `.ttc`, `.otc`) for embedding; use 0 for a file with one font.
    ///
    /// The file is read into memory once; it may change or go away afterwards.
    ///
    /// ```no_run
    /// use officewright_pdf::{Document, Font, PageSize, Pt};
    ///
    /// let font = Font::from_file("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0)?;
    /// let mut doc = Document::new();
    /// doc.add_page(PageSize::A4)?
    ///     .add_text(Pt(72.0), Pt(770.0), &font, Pt(12.0), "Привет, мир")?;
    /// doc.save("hello.pdf")?;
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Read`] if the file cannot be read, and the errors of
    /// [`from_face`](Self::from_face): [`Error::Font`] if it is not a font, has no such
    /// face, is damaged, is past a limit of the library or has outlines that cannot be
    /// embedded, and
    /// [`Error::EmbeddingNotAllowed`] if its license forbids embedding.
    pub fn from_file(path: impl AsRef<Path>, face_index: u32) -> Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let face = FontFace::from_bytes(data, face_index).map_err(|source| Error::Font {
            path: Some(path.to_owned()),
            source,
        })?;
        Self::embed(face, Some(path))
    }

    /// Reads face `face_index` of a TrueType or OpenType font or font collection in
    /// memory for embedding; use 0 for data with one font.
    ///
    /// # Errors
    ///
    /// As [`from_file`](Self::from_file), apart from reading the file.
    pub fn from_bytes(data: impl Into<Arc<[u8]>>, face_index: u32) -> Result<Self> {
        let face = FontFace::from_bytes(data, face_index)
            .map_err(|source| Error::Font { path: None, source })?;
        Self::embed(face, None)
    }

    /// A face read by `officewright-font`, for example one a
    /// [`FontResolver`](officewright_font::FontResolver) chose, for embedding.
    ///
    /// The license recorded in the font (`fsType` in its `OS/2` table) decides how it
    /// is embedded: a font that must not be embedded, or whose outlines must not be, is
    /// refused; one that must not be subset is embedded whole. Every restriction that
    /// does not stop embedding is listed in [`embedding_notes`](Self::embedding_notes).
    ///
    /// # Errors
    ///
    /// [`Error::EmbeddingNotAllowed`] if the license forbids embedding the outlines, and
    /// [`Error::Font`] if the face has no TrueType or CFF outlines (CFF2 cannot be
    /// embedded yet), has CID-keyed CFF outlines with more Font DICTs than the subsetter
    /// takes (255), or if subsetting finds it damaged or past a limit of the library
    /// (`name` records that point at more than 1 MiB of strings).
    pub fn from_face(face: FontFace) -> Result<Self> {
        Self::embed(face, None)
    }

    /// The font a [`FontResolver`] chooses for `request`, for embedding: the requested
    /// family, or the substitute or fallback the resolver takes when it is not in its
    /// [`FontBook`](officewright_font::FontBook).
    ///
    /// Every substitution is added to the resolver's
    /// [`substitutions`](FontResolver::substitutions), the report to show next to the
    /// output; the font's own [`embedding_notes`](Self::embedding_notes) report what its
    /// license restricts. A style the face lacks is reported as synthetic bold or
    /// italic but not drawn yet: the face is used as it is.
    ///
    /// ```no_run
    /// use officewright_font::{FontBook, FontRequest, FontResolver};
    /// use officewright_pdf::Font;
    ///
    /// let mut book = FontBook::new();
    /// book.add_system_fonts();
    /// let mut resolver = FontResolver::new(&book);
    /// let font = Font::resolve(&mut resolver, &FontRequest::new("Calibri"))?;
    /// for substitution in resolver.substitutions() {
    ///     eprintln!("{substitution}"); // such as "Calibri" → "Carlito" (metric-compatible substitute)
    /// }
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::FontNotFound`] if the book has no font the resolver could take, and the
    /// errors of [`from_face`](Self::from_face) for the face it chose, with
    /// [`Error::Font`] also if the face's file cannot be read any more.
    pub fn resolve(resolver: &mut FontResolver<'_>, request: &FontRequest) -> Result<Self> {
        let Some(id) = resolver.resolve(request).face() else {
            return Err(Error::FontNotFound {
                family: request.family().to_owned(),
            });
        };
        let book = resolver.book();
        let path = book.face(id).and_then(|face| face.path());
        let face = book.load(id).map_err(|source| Error::Font {
            path: path.map(Path::to_owned),
            source,
        })?;
        Self::embed(face, path)
    }

    fn embed(face: FontFace, path: Option<&Path>) -> Result<Self> {
        let name = postscript_name(&face);
        let permissions = face.embedding();
        if !permissions.allows_embedding() || permissions.bitmap_only() {
            return Err(Error::EmbeddingNotAllowed {
                font: name,
                bitmap_only: permissions.allows_embedding() && permissions.bitmap_only(),
            });
        }
        // Subsetting to no glyphs checks now that the outlines can be embedded, rather
        // than when a document is written.
        face.subset([]).map_err(|source| Error::Font {
            path: path.map(Path::to_owned),
            source,
        })?;
        let mut notes = Vec::new();
        match permissions.license() {
            EmbeddingLicense::PreviewAndPrint => notes.push(EmbeddingNote::PreviewAndPrint),
            EmbeddingLicense::Editable => notes.push(EmbeddingNote::Editable),
            _ => {}
        }
        let whole = !permissions.allows_subsetting();
        if whole {
            notes.push(EmbeddingNote::WholeFont);
        }
        if face.is_variable() {
            notes.push(EmbeddingNote::DefaultInstance);
        }
        // The probe above refuses outlines other than TrueType and CFF.
        let cff = face.outline_format() == Some(OutlineFormat::Cff);
        let name = embed::fit_font_name(&name, whole, cff);
        Ok(Self {
            kind: Kind::Embedded(Arc::new(Embedded {
                face,
                name,
                whole,
                notes,
            })),
        })
    }

    /// The font's name: the standard font's name, such as "Helvetica", or an embedded
    /// font's PostScript name, the name it has in the PDF.
    ///
    /// A PostScript name too long for the PDF is cut between characters: with the
    /// six-letter tag a subset's name starts with, and the "-Identity-H" a CFF font's
    /// Type 0 font adds, every name of the font in the file takes at most the 127 bytes
    /// readers have to accept (ISO 32000-1, Annex C). Null characters, which no PDF name
    /// can hold (7.3.5), are left out first.
    pub fn name(&self) -> &str {
        match &self.kind {
            Kind::Standard(StandardFont::Helvetica) => "Helvetica",
            Kind::Embedded(font) => &font.name,
        }
    }

    /// What the user of an embedded font should know about its license and format; empty
    /// for a standard font and for a font without restrictions.
    pub fn embedding_notes(&self) -> &[EmbeddingNote] {
        match &self.kind {
            Kind::Standard(_) => &[],
            Kind::Embedded(font) => &font.notes,
        }
    }

    /// Returns the width of `text` drawn in this font at `font_size`, as
    /// [`Page::add_text`](crate::Page::add_text) draws it: the sum of the glyphs' advance
    /// widths, without kerning.
    ///
    /// ```
    /// use officewright_pdf::{Font, Pt, StandardFont};
    ///
    /// // Helvetica's "H" is 722/1000 em wide.
    /// let font = Font::standard(StandardFont::Helvetica);
    /// assert_eq!(font.text_width("H", Pt(10.0))?, Pt(7.22));
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The errors of [`Page::add_text`](crate::Page::add_text) for a character the font
    /// cannot show, and [`Error::InvalidFontSize`] if `font_size` is outside
    /// [`MIN_SIZE`](Self::MIN_SIZE) to [`MAX_SIZE`](Self::MAX_SIZE).
    #[doc(alias = "MeasureString")]
    #[doc(alias = "measure")]
    pub fn text_width(&self, text: &str, font_size: Pt) -> Result<Pt> {
        check_font_size(font_size)?;
        let lookup = self.lookup();
        let mut units = 0.0;
        for (offset, ch) in text.char_indices() {
            units += lookup.glyph(ch, offset)?.advance;
        }
        Ok(Pt(units * font_size.0 / 1000.0))
    }

    /// Looks up the glyphs of characters; one lookup serves a whole text.
    pub(crate) fn lookup(&self) -> Lookup<'_> {
        match &self.kind {
            Kind::Standard(font) => Lookup::Standard(*font),
            Kind::Embedded(font) => Lookup::Embedded {
                font,
                map: font.face.char_map(),
                scale: 1000.0 / f64::from(font.face.units_per_em()),
            },
        }
    }

    /// The embedded font, `None` for a standard font.
    pub(crate) fn embedded(&self) -> Option<&Embedded> {
        match &self.kind {
            Kind::Standard(_) => None,
            Kind::Embedded(font) => Some(font),
        }
    }

    /// The font dictionary of a standard font, its object in the PDF file.
    pub(crate) fn standard_dictionary(&self) -> Option<&'static [u8]> {
        match self.kind {
            Kind::Standard(StandardFont::Helvetica) => Some(
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
                  /Encoding /WinAnsiEncoding >>",
            ),
            Kind::Embedded(_) => None,
        }
    }

    /// Encodes `text` in a standard font's encoding.
    pub(crate) fn encode(&self, text: &str) -> Result<Vec<u8>> {
        text.char_indices()
            .map(|(offset, ch)| encoding::win_ansi(ch).ok_or(Error::Unencodable { ch, offset }))
            .collect()
    }
}

/// The glyphs of a font's characters.
pub(crate) enum Lookup<'a> {
    Standard(StandardFont),
    Embedded {
        font: &'a Embedded,
        map: CharMap<'a>,
        /// From font units to 1/1000 em.
        scale: f64,
    },
}

/// A character's glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glyph {
    /// The WinAnsiEncoding code for a standard font, the glyph ID for an embedded one.
    pub(crate) id: u16,
    /// The advance width in 1/1000 em.
    pub(crate) advance: f64,
}

impl Lookup<'_> {
    /// The glyph of `ch`, which is at byte `offset` of its text.
    ///
    /// # Errors
    ///
    /// [`Error::Unencodable`] for a character outside a standard font's encoding;
    /// [`Error::UnsupportedCharacter`] and [`Error::MissingGlyph`] for one an embedded
    /// font cannot show.
    pub(crate) fn glyph(&self, ch: char, offset: usize) -> Result<Glyph> {
        match self {
            Self::Standard(StandardFont::Helvetica) => {
                let code = encoding::win_ansi(ch).ok_or(Error::Unencodable { ch, offset })?;
                Ok(Glyph {
                    id: code.into(),
                    advance: helvetica::width(code).into(),
                })
            }
            Self::Embedded { font, map, scale } => {
                script::check(ch).map_err(|_| Error::UnsupportedCharacter { ch, offset })?;
                let glyph = map.map(ch).ok_or_else(|| Error::MissingGlyph {
                    ch,
                    offset,
                    font: font.name.clone(),
                })?;
                let advance = font.face.advance_width(glyph).unwrap_or(0);
                Ok(Glyph {
                    id: glyph.to_u16(),
                    advance: f64::from(advance) * scale,
                })
            }
        }
    }
}

/// The face's PostScript name (name ID 6), or one made from its family and subfamily
/// names without spaces, as PostScript names are, if it has none.
fn postscript_name(face: &FontFace) -> String {
    let names = face.names();
    if let Some(name) = names.postscript_name() {
        return name.to_owned();
    }
    let made: String = [names.family(), names.subfamily()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("-")
        .split_whitespace()
        .collect();
    if made.is_empty() {
        format!("Font{}", face.index())
    } else {
        made
    }
}

/// Fails unless `size` is within [`Font::MIN_SIZE`] to [`Font::MAX_SIZE`].
pub(crate) fn check_font_size(size: Pt) -> Result<()> {
    if (Font::MIN_SIZE.0..=Font::MAX_SIZE.0).contains(&size.0) {
        Ok(())
    } else {
        Err(Error::InvalidFontSize(size))
    }
}
