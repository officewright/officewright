//! The document rendered into PDF: [`Document::save_pdf`], [`Document::save_pdf_path`]
//! and [`Document::save_pdf_bytes`].

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use officewright_font::{FontBook, FontFace, FontOrigin, FontRequest, ResolutionKind};
use officewright_layout::{LayoutReport, Location, RenderOptions, ReportEntry, ReportKind};

use crate::docx::shorten;
use crate::error::Result;
use crate::model::Document;

impl Document {
    /// Renders the document into PDF and writes it to `out`. Returns the report of what
    /// the PDF shows otherwise than Word would, so that users know why it differs.
    ///
    /// The document is translated into the layout engine's input, laid out on pages and
    /// painted. Its text is
    /// laid out with the fonts the document embeds first, then those of `options`, then
    /// the system's when `options` use them. The report lists, in this order, what the
    /// translation leaves out, the embedded fonts that cannot be read or used, and what
    /// the layout does differently from Word, such as a font it uses in place of another.
    ///
    /// An embedded font is used for the text whose font the font book finds it by: its
    /// family, typographic family, full or PostScript name, or a named instance. A subset
    /// Word embeds under a name of its own (`___WRD_EMBED_SUB_` and a number) is found by
    /// none, so other fonts are used, and the report says so (#216). At most 1,024 faces
    /// of embedded fonts are used; the report counts those left out.
    ///
    /// # Errors
    ///
    /// - [`Error::Layout`](crate::Error::Layout) when the document takes more pages than
    ///   `options` allow ([`RenderOptions::with_max_pages`]);
    /// - [`Error::Font`](crate::Error::Font) when a font `options` give cannot be read, or
    ///   a directory of fonts cannot be listed;
    /// - [`Error::Pdf`](crate::Error::Pdf) when the PDF cannot be written to `out`, or
    ///   cannot be made, such as when a font the document embeds is damaged past what the
    ///   font book checks and cannot be subset for the PDF (#217).
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Document, RenderOptions};
    ///
    /// let document = Document::open("tests/data/hello.docx")?;
    /// let mut pdf = Vec::new();
    /// let report = document.save_pdf(&mut pdf, &RenderOptions::new())?;
    /// assert!(pdf.starts_with(b"%PDF-"));
    /// for entry in report.entries() {
    ///     println!("{entry}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn save_pdf(&self, out: impl Write, options: &RenderOptions) -> Result<LayoutReport> {
        let (pdf, report) = self.render(options)?;
        pdf.write_to(out)?;
        Ok(report)
    }

    /// Renders the document into PDF and writes it to the file at `path`, as
    /// [`save_pdf`](Self::save_pdf) does; returns its report.
    ///
    /// # Errors
    ///
    /// Those of [`save_pdf`](Self::save_pdf).
    ///
    /// ```no_run
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Document, RenderOptions};
    ///
    /// let document = Document::open("report.docx")?;
    /// let options = RenderOptions::new().with_font_dir("fonts");
    /// let report = document.save_pdf_path("report.pdf", &options)?;
    /// println!("{} differences from Word", report.len());
    /// # Ok(())
    /// # }
    /// ```
    pub fn save_pdf_path(
        &self,
        path: impl AsRef<Path>,
        options: &RenderOptions,
    ) -> Result<LayoutReport> {
        let (pdf, report) = self.render(options)?;
        pdf.save(path)?;
        Ok(report)
    }

    /// Renders the document into PDF in memory, as [`save_pdf`](Self::save_pdf) does;
    /// returns the PDF's bytes and its report.
    ///
    /// # Errors
    ///
    /// Those of [`save_pdf`](Self::save_pdf).
    ///
    /// ```
    /// # fn main() -> officewright_doc::Result<()> {
    /// use officewright_doc::{Document, RenderOptions};
    ///
    /// let document = Document::open("tests/data/hello.docx")?;
    /// // Only the fonts the document embeds, none of the system's.
    /// let options = RenderOptions::new().with_system_fonts(false);
    /// let (pdf, report) = document.save_pdf_bytes(&options)?;
    /// assert!(pdf.starts_with(b"%PDF-"));
    /// // Without a font for its text, the report says so.
    /// assert!(!report.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn save_pdf_bytes(&self, options: &RenderOptions) -> Result<(Vec<u8>, LayoutReport)> {
        let (pdf, report) = self.render(options)?;
        Ok((pdf.to_bytes()?, report))
    }

    /// The document rendered into PDF, and the report of the translation, the embedded
    /// fonts and the layout.
    fn render(
        &self,
        options: &RenderOptions,
    ) -> Result<(officewright_pdf::Document, LayoutReport)> {
        let (document, mut report) = self.to_layout_ir().into_parts();
        let mut fonts = options.font_book()?;
        report.extend(self.add_embedded_fonts(&mut fonts));
        let (pdf, rendered) =
            officewright_layout::render_pdf(&document, &fonts, options)?.into_parts();
        report.extend(rendered);
        Ok((pdf, report))
    }

    /// Adds the programs of the fonts the document embeds to `fonts`, ahead of the
    /// caller's and the system's, each program once however many styles share it, and
    /// at most [`MAX_EMBEDDED_FACES`] faces in all. Returns an entry for each program the
    /// font book cannot read, which is left out; for each the text cannot reach by the
    /// font's name; and one for those past the limit, which are left out.
    fn add_embedded_fonts(&self, fonts: &mut FontBook) -> Vec<ReportEntry> {
        let mut added = HashSet::new();
        let mut entries = Vec::new();
        let (mut faces, mut past_limit) = (0usize, 0usize);
        for font in self.fonts() {
            for embedded in font.embedded() {
                let Some(program) = &embedded.program else {
                    continue;
                };
                if !added.insert(Arc::as_ptr(&program.0).cast::<u8>()) {
                    continue;
                }
                let what = || {
                    format!(
                        "the w:{} of the font {}",
                        embedded.style().to_xml(),
                        shorten(font.name())
                    )
                };
                // A collection of a thousand faces takes a few kilobytes: the faces, not
                // the bytes, are what costs.
                let count = FontFace::face_count(&program.0).map_or(1, |count| count as usize);
                if faces.saturating_add(count) > MAX_EMBEDDED_FACES {
                    past_limit += 1;
                    continue;
                }
                faces += count;
                match fonts.add_bytes(Arc::clone(&program.0), FontOrigin::Document) {
                    Ok(_) => {
                        if let Some(family) = unreachable_family(&program.0, font.name()) {
                            entries.push(ReportEntry::new(
                                ReportKind::FontSubstituted,
                                Location::document(),
                                format!(
                                    "{} holds {family}, which the text cannot reach by the \
                                     font's name, so other fonts are used",
                                    what()
                                ),
                            ));
                        }
                    }
                    Err(err) => {
                        // The core error's message keeps the font parser's reason.
                        let reason = officewright_core::Error::from(err);
                        entries.push(ReportEntry::new(
                            ReportKind::FontEmbedding,
                            Location::document(),
                            format!(
                                "{} cannot be read, so other fonts are used: {reason}",
                                what()
                            ),
                        ));
                    }
                }
            }
        }
        if past_limit > 0 {
            entries.push(ReportEntry::new(
                ReportKind::FontEmbedding,
                Location::document(),
                format!(
                    "{past_limit} embedded font programs are left out, past {MAX_EMBEDDED_FACES} \
                     faces in all, so other fonts are used"
                ),
            ));
        }
        entries
    }
}

/// The most faces the document's embedded fonts may add to the font book, a collection
/// counting each of its faces. Word embeds at most four programs a font; the limit is
/// our choice, far above what documents embed and small enough that a few kilobytes of
/// collections cannot fill the memory.
const MAX_EMBEDDED_FACES: usize = 1024;

/// The family `program` holds, described for the report, when the font book would not
/// find it by `name`: a font book with only the program finds no face of that name, by
/// any of the ways the font book finds names (the family, an alias, another of the
/// font's names, a named instance). A subset Word embeds under a name of its own
/// (`___WRD_EMBED_SUB_` and a number) is such a program.
fn unreachable_family(program: &Arc<[u8]>, name: &str) -> Option<String> {
    let mut alone = FontBook::new();
    alone
        .add_bytes(Arc::clone(program), FontOrigin::Document)
        .ok()?;
    let resolution = alone.resolve(&FontRequest::new(name));
    let found = matches!(
        resolution.kind(),
        ResolutionKind::Exact
            | ResolutionKind::Alias
            | ResolutionKind::OtherName
            | ResolutionKind::VariableInstance
    );
    if found {
        return None;
    }
    let family = alone
        .faces()
        .next()
        .and_then(|(_, face)| face.names().family())
        .map_or_else(
            || "a font without a family name".to_owned(),
            |family| format!("a font named {}", shorten(family)),
        );
    Some(family)
}
