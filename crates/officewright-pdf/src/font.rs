use officewright_core::Pt;

use crate::{Error, Result, encoding, helvetica};

/// A font to draw and measure text with.
///
/// For now the standard Helvetica font is the only one. It is one of the fonts every
/// PDF viewer provides, so nothing is embedded, and it is used with WinAnsiEncoding,
/// which covers Latin text only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    standard: StandardFont,
}

/// The standard fonts that PDF viewers provide without embedding (ISO 32000-1, 9.6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StandardFont {
    /// Helvetica, with WinAnsiEncoding.
    Helvetica,
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
        Self { standard: font }
    }

    /// Returns the width of `text` drawn in this font at `font_size`, as
    /// [`Page::add_text`](crate::Page::add_text) draws it (no kerning).
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
    /// [`Error::Unencodable`] if `text` has a character the font's encoding lacks, and
    /// [`Error::InvalidFontSize`] if `font_size` is outside [`MIN_SIZE`](Self::MIN_SIZE)
    /// to [`MAX_SIZE`](Self::MAX_SIZE).
    #[doc(alias = "MeasureString")]
    #[doc(alias = "measure")]
    pub fn text_width(&self, text: &str, font_size: Pt) -> Result<Pt> {
        check_font_size(font_size)?;
        let units: u64 = self
            .encode(text)?
            .into_iter()
            .map(|code| u64::from(self.width(code)))
            .sum();
        Ok(Pt(units as f64 * font_size.0 / 1000.0))
    }

    /// Fails unless the font's encoding has every character of `text`.
    pub(crate) fn check(&self, text: &str) -> Result<()> {
        match text.char_indices().find(|&(_, ch)| self.code(ch).is_none()) {
            Some((offset, ch)) => Err(Error::Unencodable { ch, offset }),
            None => Ok(()),
        }
    }

    /// The font dictionary, the font's object in the PDF file.
    pub(crate) fn dictionary(&self) -> &'static [u8] {
        match self.standard {
            StandardFont::Helvetica => {
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
                  /Encoding /WinAnsiEncoding >>"
            }
        }
    }

    /// Encodes `text` in the font's encoding.
    pub(crate) fn encode(&self, text: &str) -> Result<Vec<u8>> {
        text.char_indices()
            .map(|(offset, ch)| self.code(ch).ok_or(Error::Unencodable { ch, offset }))
            .collect()
    }

    /// The code of `ch` in the font's encoding, if it has one.
    pub(crate) fn code(&self, ch: char) -> Option<u8> {
        match self.standard {
            StandardFont::Helvetica => encoding::win_ansi(ch),
        }
    }

    /// The advance width of a character code in 1/1000 em.
    pub(crate) fn width(&self, code: u8) -> u16 {
        match self.standard {
            StandardFont::Helvetica => helvetica::width(code),
        }
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
