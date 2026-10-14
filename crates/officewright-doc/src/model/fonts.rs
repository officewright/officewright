//! The font table of a document (`fontTable.xml`, ECMA-376 Part 1, §17.8), as written:
//! what the document says about each font it uses, so that a substitute can be chosen
//! when a font is missing.

use std::sync::Arc;

use super::properties::xml_enum;

xml_enum! {
    /// The kind of design of a font (ST_FontFamily).
    FontFamily {
        /// Novelty fonts.
        Decorative = "decorative",
        /// Fixed-pitch fonts, with or without serifs.
        Modern = "modern",
        /// Proportional fonts with serifs.
        Roman = "roman",
        /// Fonts like handwriting.
        Script = "script",
        /// Proportional fonts without serifs.
        Swiss = "swiss",
        /// Not known.
        Auto = "auto",
    }
}

xml_enum! {
    /// Whether the characters of a font are all as wide (ST_Pitch).
    FontPitch {
        Fixed = "fixed",
        Variable = "variable",
        Default = "default",
    }
}

xml_enum! {
    /// Which style of a font an embedded font is.
    EmbeddedFontStyle {
        Regular = "embedRegular",
        Bold = "embedBold",
        Italic = "embedItalic",
        BoldItalic = "embedBoldItalic",
    }
}

/// The Unicode ranges and code pages a font covers (`w:sig`), as the OS/2 table of a
/// font gives them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontSignature {
    pub(crate) unicode: [u32; 4],
    pub(crate) code_pages: [u32; 2],
}

impl FontSignature {
    /// The Unicode range bits (`w:usb0` to `w:usb3`).
    pub fn unicode_ranges(&self) -> [u32; 4] {
        self.unicode
    }

    /// The code page bits (`w:csb0`, `w:csb1`).
    pub fn code_pages(&self) -> [u32; 2] {
        self.code_pages
    }
}

/// A font embedded in the package for one style (`w:embedRegular` and the like): the
/// relationship to its part and the key it is obfuscated with, as written, and the font
/// program read from the part.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmbeddedFontReference {
    pub(crate) style: EmbeddedFontStyle,
    pub(crate) relationship_id: Box<str>,
    pub(crate) key: Option<Box<str>>,
    pub(crate) subsetted: Option<bool>,
    pub(crate) program: Option<Program>,
}

/// The bytes of an embedded font program, shared by the fonts that name one part with
/// one key; debug output gives their number only.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Program(pub(crate) Arc<[u8]>);

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Program({} bytes)", self.0.len())
    }
}

impl EmbeddedFontReference {
    /// The style it is.
    pub fn style(&self) -> EmbeddedFontStyle {
        self.style
    }

    /// The id of the relationship of the font table to the font's part (`r:id`).
    pub fn relationship_id(&self) -> &str {
        &self.relationship_id
    }

    /// The key the font's bytes are obfuscated with (`w:fontKey`), a GUID in braces.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Only the characters the document uses are embedded (`w:subsetted`).
    pub fn subsetted(&self) -> Option<bool> {
        self.subsetted
    }

    /// The font program, TrueType or OpenType, as the part holds it once its first 32
    /// bytes are no longer obfuscated (ECMA-376 Part 1, §17.8.1). It is read when the
    /// document is opened. None, with a diagnostic, when the font table's relationships
    /// or the package do not have the part, the part cannot be read, the key is not a
    /// GUID, or the bytes are not a font with or without the key; none, too, for a font
    /// outside the package.
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::Document;
    ///
    /// let document = Document::open("tests/data/hello.docx")?;
    /// for font in document.fonts() {
    ///     for embedded in font.embedded() {
    ///         if let Some(program) = embedded.program() {
    ///             println!("{} {:?}: {} bytes", font.name(), embedded.style(), program.len());
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn program(&self) -> Option<&[u8]> {
        self.program.as_ref().map(|program| &*program.0)
    }
}

/// What the font table says about one font (`w:font`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontDeclaration {
    pub(crate) name: Box<str>,
    pub(crate) alternative_name: Option<Box<str>>,
    pub(crate) panose: Option<[u8; 10]>,
    pub(crate) charset: Option<u8>,
    pub(crate) character_set: Option<Box<str>>,
    pub(crate) family: Option<FontFamily>,
    pub(crate) pitch: Option<FontPitch>,
    pub(crate) signature: Option<FontSignature>,
    pub(crate) not_true_type: Option<bool>,
    pub(crate) embedded: Vec<EmbeddedFontReference>,
}

impl FontDeclaration {
    /// The font's name, as runs name it (`w:name`). Word allows 1 to 31 characters, and
    /// writes a name with `@` before it for the font of vertical text (MS-OI29500
    /// §17.8.3.10(a), (c)); a name is kept as written, leading spaces too.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Another name of the font (`w:altName`), such as its name in another language. The
    /// standard calls it a comma-separated list, but Word takes the whole value as one
    /// name of at most 31 characters (MS-OI29500 §17.8.3.1(a), (b)).
    pub fn alternative_name(&self) -> Option<&str> {
        self.alternative_name.as_deref()
    }

    /// The PANOSE classification of the font's design (`w:panose1`), ten bytes.
    pub fn panose(&self) -> Option<[u8; 10]> {
        self.panose
    }

    /// The character set of the font, as a Windows charset number (`w:charset`
    /// `w:val`): 0 for ANSI, 2 for symbol fonts, 128 for Shift JIS and so on.
    pub fn charset(&self) -> Option<u8> {
        self.charset
    }

    /// The character set by its IANA name (`w:charset` `w:characterSet`). Word ignores
    /// it (MS-OI29500 §17.8.3.2(b)) and goes by [`charset`](Self::charset).
    pub fn character_set(&self) -> Option<&str> {
        self.character_set.as_deref()
    }

    /// The kind of design (`w:family`).
    pub fn family(&self) -> Option<FontFamily> {
        self.family
    }

    /// Whether the characters are all as wide (`w:pitch`).
    pub fn pitch(&self) -> Option<FontPitch> {
        self.pitch
    }

    /// The Unicode ranges and code pages the font covers (`w:sig`); `None` unless all six
    /// of its values are written and valid.
    pub fn signature(&self) -> Option<&FontSignature> {
        self.signature.as_ref()
    }

    /// The font is not a TrueType or OpenType font (`w:notTrueType`).
    pub fn not_true_type(&self) -> Option<bool> {
        self.not_true_type
    }

    /// The styles of the font embedded in the package, in document order.
    pub fn embedded(&self) -> &[EmbeddedFontReference] {
        &self.embedded
    }
}
