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
    /// none, so other fonts are used, and the report says so (#216). At most 256 faces
    /// of embedded fonts are used, and no program with a name longer than any font's
    /// (#226); the report says what is left out.
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
    /// at most [`MAX_EMBEDDED_FACES`] faces in all. Returns an entry for each program
    /// left out, because it cannot be read or has a name longer than any font's, and for
    /// each the text cannot reach by the font's name; and one for those past the limit.
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
                // The font book keeps each name a face has, however long; a document's
                // program could hold names of megabytes. Until it bounds them (#226),
                // such a program does not reach a font book.
                if has_long_name(&program.0) {
                    entries.push(ReportEntry::new(
                        ReportKind::FontEmbedding,
                        Location::document(),
                        format!(
                            "{} has a name longer than {MAX_NAME_BYTES} bytes, which no font has, \
                             so it is left out and other fonts are used",
                            what()
                        ),
                    ));
                    continue;
                }
                // Whether the text reaches the program, asked of a book that holds only
                // it, which is gone before the program goes into `fonts`.
                let reach = reaches(&program.0, font.name());
                match fonts.add_bytes(Arc::clone(&program.0), FontOrigin::Document) {
                    Ok(_) => {
                        if let Some(Err(family)) = reach {
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
            let programs = if past_limit == 1 {
                "program is"
            } else {
                "programs are"
            };
            entries.push(ReportEntry::new(
                ReportKind::FontEmbedding,
                Location::document(),
                format!(
                    "{past_limit} embedded font {programs} left out, past {MAX_EMBEDDED_FACES} \
                     faces in all, so other fonts are used"
                ),
            ));
        }
        entries
    }
}

/// The most faces the document's embedded fonts may add to the font book, a collection
/// counting each of its faces. Word embeds at most four programs a font; the limit is
/// our choice, far above what documents embed (24 programs in all in the corpora) and
/// small enough that collections of a few kilobytes cannot fill the memory.
const MAX_EMBEDDED_FACES: usize = 256;

/// The longest name, in bytes, that an embedded program may have among those the font
/// book keeps. Real fonts' are short: Windows allows 31 characters in a face name and
/// OpenType 63 in a PostScript name. Our choice, until the font book bounds them (#226).
const MAX_NAME_BYTES: u16 = 512;

/// Whether a face of `program` has a name the font book keeps longer than
/// [`MAX_NAME_BYTES`]: its family, subfamily, full, PostScript and typographic names,
/// those of IDs 21, 22 and 256 and up, and the name each named instance of a variable
/// font gives as its subfamily, whatever its ID (OpenType's `fvar` table). Only the
/// lengths of the name records are read (the `name` table), not the names; a face
/// whose tables cannot be found has none, and the font book says why it cannot read
/// it.
fn has_long_name(program: &[u8]) -> bool {
    let u16_at = |at: Option<usize>| {
        let at = at?;
        let bytes = program.get(at..at.checked_add(2)?)?;
        <[u8; 2]>::try_from(bytes).ok().map(u16::from_be_bytes)
    };
    let u32_at = |at: Option<usize>| {
        let at = at?;
        let bytes = program.get(at..at.checked_add(4)?)?;
        <[u8; 4]>::try_from(bytes)
            .ok()
            .map(|bytes| u32::from_be_bytes(bytes) as usize)
    };
    // `base + step * index + extra`, none past the address space.
    let place = |base: usize, step: usize, index: usize, extra: usize| {
        step.checked_mul(index)?
            .checked_add(base)?
            .checked_add(extra)
    };
    // The offsets of the faces' table directories: one for a font, each of a collection's.
    let faces: Vec<usize> = if program.starts_with(b"ttcf") {
        let count = u32_at(Some(8)).unwrap_or(0);
        (0..count.min(MAX_EMBEDDED_FACES))
            .filter_map(|face| u32_at(place(12, 4, face, 0)))
            .collect()
    } else {
        vec![0]
    };
    let mut seen = HashSet::new();
    for face in faces {
        let tables = usize::from(u16_at(face.checked_add(4)).unwrap_or(0));
        let table = |wanted: &[u8; 4]| {
            (0..tables).find_map(|table| {
                let record = place(face, 16, table, 12)?;
                let tag = program.get(record..record.checked_add(4)?)?;
                if tag == wanted {
                    u32_at(record.checked_add(8))
                } else {
                    None
                }
            })
        };
        let Some(name) = table(b"name") else {
            continue;
        };
        // The IDs of the names the named instances of a variable font give as their
        // subfamily, which the font book keeps too (the first 256 of them).
        let mut instance_ids = HashSet::new();
        if let Some(fvar) = table(b"fvar") {
            let field = |at: usize| u16_at(fvar.checked_add(at)).map(usize::from);
            if let (Some(axes), Some(axis_count), Some(axis_size), Some(count), Some(size)) =
                (field(4), field(8), field(10), field(12), field(14))
            {
                let first = axis_count
                    .checked_mul(axis_size)
                    .and_then(|axes_size| fvar.checked_add(axes)?.checked_add(axes_size));
                for instance in 0..count.min(256) {
                    match first.and_then(|first| u16_at(place(first, size, instance, 0))) {
                        Some(id) => instance_ids.insert(id),
                        None => break,
                    };
                }
            }
        }
        // The faces of a collection often share one name table and its instances.
        let mut key: Vec<u16> = instance_ids.iter().copied().collect();
        key.sort_unstable();
        if !seen.insert((name, key)) {
            continue;
        }
        let count = usize::from(u16_at(name.checked_add(2)).unwrap_or(0));
        for record in 0..count {
            let (Some(id), Some(length)) = (
                u16_at(place(name, 12, record, 6 + 6)),
                u16_at(place(name, 12, record, 6 + 8)),
            ) else {
                break;
            };
            let kept = matches!(id, 1 | 2 | 4 | 6 | 16 | 17 | 21 | 22)
                || id >= 256
                || instance_ids.contains(&id);
            if kept && length > MAX_NAME_BYTES {
                return true;
            }
        }
    }
    false
}

/// Whether the font book would find `program` by `name`: `Some(Ok(()))` when a font
/// book holding only the program finds a face by its family, an alias, another of its
/// names or a named instance, or takes it for a metric-compatible substitute of the
/// name; `Some(Err(family))` with the family the program holds, described for the
/// report, when it does not, as for a subset Word embeds under a name of its own
/// (`___WRD_EMBED_SUB_` and a number); none when no font book can read it.
fn reaches(program: &Arc<[u8]>, name: &str) -> Option<std::result::Result<(), String>> {
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
            | ResolutionKind::MetricCompatible
    );
    if found {
        return Some(Ok(()));
    }
    let family = alone
        .faces()
        .next()
        .and_then(|(_, face)| face.names().family())
        .map_or_else(
            || "a font without a family name".to_owned(),
            |family| format!("a font named {}", shorten(family)),
        );
    Some(Err(family))
}
