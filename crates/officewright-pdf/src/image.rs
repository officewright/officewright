//! Raster images: JPEG files passed through with the DCTDecode filter (some damaged ones
//! cut after their only scan), and PNG files passed
//! through with the PNG predictors of the FlateDecode filter where PDF can show them as
//! they are, decoded otherwise (ISO 32000-1, 8.9 and 7.4.4.4).

use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, fs};

use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_zlib_with_limit};

use crate::document::number;
use crate::stream::stream_object;
use crate::{Compression, Error, Result};

/// The most bytes of pixel data a PNG may have once decompressed, and once decoded: 256
/// MiB, room for an RGBA image of 67 million pixels while a document stays well within a
/// gigabyte of memory.
pub(crate) const MAX_PNG_DATA: u64 = 1 << 28;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A raster image to draw on pages with [`Page::draw_image`](crate::Page::draw_image): a
/// JPEG or PNG file.
///
/// A JPEG file goes into the PDF as it is, except a sequential one whose only scan is
/// followed by what libjpeg fails on: that goes in up to the end of the scan, with an end of
/// image, which draws the same picture. So does a PNG file whenever PDF can show it
/// unchanged: greyscale, colour or palette images without interlacing, with no
/// transparency or with fully transparent palette entries that form one range. Other PNG
/// files, with an alpha channel, other transparency or interlacing, are decoded, and
/// their transparency becomes a soft mask; 16-bit samples are then reduced to 8 bits.
/// Colour profiles and gamma are ignored, as Word's PDF export ignores them; colours are
/// written as sRGB device colours.
///
/// Cloning is cheap: the image data is shared. Two images are equal if they were read
/// from the same data, and a document embeds each image once however often it is drawn.
/// An `Image` is `Send` and `Sync`.
///
/// One of the drawing primitives the layout engine paints with: public, but exempt from
/// semantic versioning until officewright 1.0 (feature `unstable-drawing`).
///
/// ```
/// use officewright_pdf::{Document, Image, PageSize, Pt};
///
/// // A 1 × 1 PNG with one red pixel.
/// let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0\x90wS\xde\
///     \0\0\0\x0cIDATx\xdac\xf8\xcf\xc0\0\0\x03\x01\x01\0\xf7\x03AC\
///     \0\0\0\0IEND\xaeB`\x82";
/// let image = Image::from_bytes(&png[..])?;
/// assert_eq!((image.width(), image.height()), (1, 1));
///
/// let mut doc = Document::new();
/// doc.add_page(PageSize::A4)?
///     .draw_image(&image, Pt(72.0), Pt(600.0), Pt(144.0), Pt(144.0))?;
/// # Ok::<(), officewright_pdf::Error>(())
/// ```
#[derive(Clone)]
pub struct Image {
    inner: Arc<Inner>,
}

/// Shows the image's size, encoding and the length of its file, not the file's bytes.
impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) || self.inner.source == other.inner.source
    }
}

impl Eq for Image {}

struct Inner {
    /// The file, which tells images apart.
    source: Arc<[u8]>,
    width: u32,
    height: u32,
    encoding: Encoding,
}

impl Inner {
    /// The compressed data of a PNG file, joined; borrowed from the file when it is one
    /// chunk, as it usually is, or from the copy.
    fn png_data<'a>(&'a self, data: &'a PngData) -> Cow<'a, [u8]> {
        match data {
            PngData::Copied(data) => Cow::Borrowed(data),
            PngData::InFile(chunks) => match chunks.as_slice() {
                [chunk] => Cow::Borrowed(self.source.get(chunk.clone()).unwrap_or_default()),
                chunks => Cow::Owned(
                    chunks
                        .iter()
                        .filter_map(|chunk| self.source.get(chunk.clone()))
                        .flatten()
                        .copied()
                        .collect(),
                ),
            },
        }
    }
}

/// The zlib stream of a PNG file's `IDAT` chunks: where the data of each non-empty chunk
/// is in the file, which is kept anyway, or a copy, once the places of the chunks read so
/// far would take more than a quarter of their data. That is decided as reading goes, so
/// a file whose first chunks are small is copied even if its later ones are large.
enum PngData {
    InFile(Vec<Range<usize>>),
    Copied(Vec<u8>),
}

impl PngData {
    /// The bytes of the zlib stream.
    fn len(&self) -> usize {
        match self {
            Self::InFile(chunks) => chunks.iter().map(ExactSizeIterator::len).sum(),
            Self::Copied(data) => data.len(),
        }
    }
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("bytes", &self.source.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("encoding", &self.encoding)
            .finish()
    }
}

/// How an image goes into a PDF.
enum Encoding {
    /// The JPEG file as it is, or up to `end` and then an end of image.
    Jpeg {
        components: u8,
        /// The file has Adobe's `APP14` marker, so its CMYK samples are inverted.
        adobe: bool,
        /// Where the coded data of the only scan of a sequential file ends, if what
        /// follows it is left out ([`read_jpeg`]).
        end: Option<usize>,
    },
    /// The zlib stream of a PNG file's `IDAT` chunks, whose rows the PNG predictors undo.
    Png {
        data: PngData,
        space: ColorSpace,
        bits: u8,
        /// The range of palette indices that are fully transparent.
        mask: Option<(u8, u8)>,
    },
    /// Decoded samples, 8 bits each, and the alpha of each pixel unless all are opaque.
    Decoded {
        samples: Vec<u8>,
        space: ColorSpace,
        alpha: Option<Vec<u8>>,
    },
}

impl fmt::Debug for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jpeg {
                components,
                adobe,
                end,
            } => f
                .debug_struct("Jpeg")
                .field("components", components)
                .field("adobe", adobe)
                .field("end", end)
                .finish(),
            Self::Png {
                data,
                space,
                bits,
                mask,
            } => f
                .debug_struct("Png")
                .field("data", &data.len())
                .field("space", space)
                .field("bits", bits)
                .field("mask", mask)
                .finish(),
            Self::Decoded {
                samples,
                space,
                alpha,
            } => f
                .debug_struct("Decoded")
                .field("samples", &samples.len())
                .field("space", space)
                .field("alpha", &alpha.is_some())
                .finish(),
        }
    }
}

/// The colour space of an image's samples.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    /// Indices into a palette of RGB colours, with as many entries as the bit depth can
    /// address.
    Indexed(Vec<u8>),
}

impl ColorSpace {
    /// The number of samples per pixel.
    fn components(&self) -> u8 {
        match self {
            Self::Gray | Self::Indexed(_) => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    /// The `/ColorSpace` entry.
    fn entry(&self) -> String {
        match self {
            Self::Gray => "/DeviceGray".to_owned(),
            Self::Rgb => "/DeviceRGB".to_owned(),
            Self::Cmyk => "/DeviceCMYK".to_owned(),
            Self::Indexed(palette) => {
                let hex: String = palette.iter().map(|byte| format!("{byte:02X}")).collect();
                format!("[/Indexed /DeviceRGB {} <{hex}>]", palette.len() / 3 - 1)
            }
        }
    }
}

impl Image {
    /// Reads a JPEG or PNG file.
    ///
    /// The file is read into memory once; it may change or go away afterwards.
    ///
    /// # Errors
    ///
    /// [`Error::Read`] if the file cannot be read, and the errors of
    /// [`from_bytes`](Self::from_bytes), which name the file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::read(data.into(), Some(path))
    }

    /// Reads a JPEG or PNG file in memory.
    ///
    /// # Errors
    ///
    /// - [`Error::UnsupportedImage`] for data in another format, and for JPEG and PNG
    ///   features PDF cannot show: lossless, hierarchical and arithmetic-coded JPEG,
    ///   12-bit JPEG, JPEG with 2 components or a height given only after the first
    ///   scan, PNG with a compression or filter method other than the standard one or
    ///   with a critical chunk this reader does not know.
    /// - [`Error::MalformedImage`] for a damaged file: truncated, with a wrong checksum
    ///   of a critical PNG chunk, missing or invalid headers, or compressed data that
    ///   does not decompress to the size its header gives.
    /// - [`Error::ImageTooLarge`] for a PNG whose pixel data would exceed 256 MiB.
    pub fn from_bytes(data: impl Into<Arc<[u8]>>) -> Result<Self> {
        Self::read(data.into(), None)
    }

    fn read(source: Arc<[u8]>, path: Option<&Path>) -> Result<Self> {
        let path = || path.map(Path::to_owned);
        let (width, height, encoding) = if source.starts_with(PNG_SIGNATURE) {
            read_png(&source).map_err(|problem| problem.error(path()))?
        } else if source.starts_with(b"\xFF\xD8") {
            read_jpeg(&source).map_err(|problem| problem.error(path()))?
        } else {
            return Err(Error::UnsupportedImage {
                path: path(),
                reason: "it is neither a JPEG nor a PNG file".to_owned(),
            });
        };
        Ok(Self {
            inner: Arc::new(Inner {
                source,
                width,
                height,
                encoding,
            }),
        })
    }

    /// The width in pixels.
    pub fn width(&self) -> u32 {
        self.inner.width
    }

    /// The height in pixels.
    pub fn height(&self) -> u32 {
        self.inner.height
    }

    /// Whether the image needs PDF 1.5: 16-bit samples do.
    pub(crate) fn needs_pdf_15(&self) -> bool {
        matches!(self.inner.encoding, Encoding::Png { bits: 16, .. })
    }

    /// How many objects the image takes in a PDF: one, and one more for a soft mask.
    pub(crate) fn object_count(&self) -> usize {
        match &self.inner.encoding {
            Encoding::Decoded { alpha: Some(_), .. } => 2,
            _ => 1,
        }
    }

    /// The image's objects, the first numbered `first`: the image XObject and its soft
    /// mask, if any.
    pub(crate) fn objects(&self, first: usize, compression: Compression) -> Vec<Vec<u8>> {
        let inner = &self.inner;
        let size = format!(
            " /Type /XObject /Subtype /Image /Width {} /Height {}",
            inner.width, inner.height
        );
        match &inner.encoding {
            Encoding::Jpeg {
                components,
                adobe,
                end,
            } => {
                let space = match components {
                    1 => ColorSpace::Gray,
                    3 => ColorSpace::Rgb,
                    _ => ColorSpace::Cmyk,
                };
                // Adobe's applications write CMYK JPEG files inverted, and mark them with
                // the APP14 segment.
                let decode = if *adobe && *components == 4 {
                    " /Decode [1 0 1 0 1 0 1 0]"
                } else {
                    ""
                };
                let entries = format!(
                    " /Filter /DCTDecode{size} /ColorSpace {} /BitsPerComponent 8{decode}",
                    space.entry()
                );
                let cut;
                let data = match end.and_then(|end| inner.source.get(..end)) {
                    Some(scan) => {
                        cut = [scan, b"\xFF\xD9"].concat();
                        &cut[..]
                    }
                    None => &*inner.source,
                };
                vec![stream_object(&entries, data, Compression::None)]
            }
            Encoding::Png {
                data,
                space,
                bits,
                mask,
            } => {
                let data = inner.png_data(data);
                let mask =
                    mask.map_or_else(String::new, |(min, max)| format!(" /Mask [{min} {max}]"));
                let entries = format!(
                    " /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors {} \
                     /BitsPerComponent {bits} /Columns {} >>{size} /ColorSpace {} \
                     /BitsPerComponent {bits}{mask}",
                    space.components(),
                    inner.width,
                    space.entry()
                );
                vec![stream_object(&entries, &data, Compression::None)]
            }
            Encoding::Decoded {
                samples,
                space,
                alpha,
            } => {
                let smask = if alpha.is_some() {
                    format!(" /SMask {} 0 R", first + 1)
                } else {
                    String::new()
                };
                let entries = format!(
                    "{size} /ColorSpace {} /BitsPerComponent 8{smask}",
                    space.entry()
                );
                let mut objects = vec![stream_object(&entries, samples, compression)];
                if let Some(alpha) = alpha {
                    let entries = format!("{size} /ColorSpace /DeviceGray /BitsPerComponent 8");
                    objects.push(stream_object(&entries, alpha, compression));
                }
                objects
            }
        }
    }
}

/// What is wrong with an image, before the error names its file.
#[derive(Debug)]
enum Problem {
    Malformed(String),
    Unsupported(String),
    TooLarge,
}

impl Problem {
    fn error(self, path: Option<PathBuf>) -> Error {
        match self {
            Self::Malformed(reason) => Error::MalformedImage { path, reason },
            Self::Unsupported(reason) => Error::UnsupportedImage { path, reason },
            Self::TooLarge => Error::ImageTooLarge { path },
        }
    }
}

fn malformed(reason: impl Into<String>) -> Problem {
    Problem::Malformed(reason.into())
}

fn unsupported(reason: impl Into<String>) -> Problem {
    Problem::Unsupported(reason.into())
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the size and components of a JPEG file from its frame header (ITU T.81, B.2),
/// and whether it has Adobe's `APP14` segment. The frame header and every scan header
/// before the end of image are checked by the rules of T.81 that libjpeg enforces when it
/// reads them, so that a file whose headers a viewer cannot read is refused; after the
/// first scan, so are a second frame header or start of image, the marker of a frame that
/// cannot go into a PDF, and a second scan where libjpeg expects none, all of which
/// libjpeg refuses too ([`check_later_scans`]).
///
/// Not refused but cut: a sequential file whose first scan codes every component has no
/// other scan, and libjpeg draws it from that scan whatever follows, as poppler shows; it
/// fails on what follows only when it reads on to the end of image, as `qpdf --check`
/// does. Where what follows is refused by those rules, the file goes in up to the end of
/// its scan's coded data, followed by an end of image, which draws the same picture and
/// passes `qpdf --check`.
///
/// Not checked: libjpeg's own limits, which T.81 does not set (widths and heights over
/// 65500, sampling factors that do not divide the largest of the frame, more than 10
/// components, of which no PDF takes more than 4); whether the tables the headers name
/// are defined; the coded data; what comes after the end of image, which libjpeg does not
/// read; and what comes after the first scan once the file ends early, where the check
/// stops. A marker libjpeg does not know is refused wherever it is
/// ([`refuse_unknown_marker`]), but where libjpeg skips it: in the coded data of a scan
/// while a restart marker is still to come ([`at_restart`]).
fn read_jpeg(data: &[u8]) -> std::result::Result<(u32, u32, Encoding), Problem> {
    let mut at = 2;
    let mut frame = None;
    let mut adobe = false;
    // The restart interval in effect for the next scan, in MCUs (B.2.4.4); 0 for none.
    let mut interval = 0;
    let (frame, coded, more_scans, restarts) = loop {
        // A marker is 0xFF and a code; more 0xFF bytes may pad it (B.1.1.2). Other bytes
        // before a marker are skipped, as libjpeg skips them.
        while data.get(at).is_some_and(|&byte| byte != 0xFF) {
            at += 1;
        }
        while data.get(at) == Some(&0xFF) {
            at += 1;
        }
        let Some(&code) = data.get(at) else {
            return Err(malformed("the JPEG file ends before its first scan"));
        };
        at += 1;
        match code {
            // 0xFF 0x00 is not a marker (B.1.1.2) but a stray 0xFF byte: skipped too.
            0x00 => continue,
            // Markers without a segment: TEM and the restart markers.
            0x01 | 0xD0..=0xD7 => continue,
            0xD8 | 0xD9 => return Err(malformed("the JPEG file has no scan")),
            _ => refuse_unknown_marker(code, at - 2)?,
        }
        // A scan comes after its frame header (B.2.1).
        let scan_frame = if code == 0xDA {
            let frame = frame.take();
            Some(frame.ok_or_else(|| malformed("the JPEG file has no frame header"))?)
        } else {
            None
        };
        let length = usize::from(u16_at(data, at).ok_or_else(|| {
            malformed(format!(
                "the JPEG file ends in the length of its FF{code:02X} segment at byte {at}"
            ))
        })?);
        let segment = data
            .get(at + 2..at + length.max(2))
            .ok_or_else(|| malformed(format!("the JPEG segment at byte {at} is truncated")))?;
        // The scan header is the last before the coded data: every header has been read.
        if let Some(frame) = scan_frame {
            check_scan(segment, length, &frame)?;
            // libjpeg decides at the first scan whether more can follow: in a progressive
            // frame, or in a sequential one whose first scan codes only some components.
            let count = usize::from(segment.first().copied().unwrap_or(0));
            let more_scans = frame.progressive || count < frame.components.len();
            let restarts = scan_restarts(&frame, segment, interval);
            break (frame, at + length, more_scans, restarts);
        }
        match code {
            // Baseline, extended sequential and progressive Huffman coding, one frame
            // header in a file (B.2.1).
            0xC0..=0xC2 if frame.is_some() => {
                return Err(malformed(format!(
                    "the JPEG file has a second frame header, at byte {}",
                    at - 2
                )));
            }
            0xC0..=0xC2 => frame = Some(read_frame(segment, code == 0xC2)?),
            0xEE => adobe |= segment.starts_with(b"Adobe"),
            0xDD => interval = u16_at(segment, 0).unwrap_or(0),
            _ => refuse_other_frame(code)?,
        }
        at += length;
    };
    let components = frame.components.len();
    if frame.precision != 8 {
        return Err(unsupported(format!(
            "{}-bit JPEG cannot go into a PDF, only 8-bit",
            frame.precision
        )));
    }
    if frame.height == 0 {
        return Err(unsupported(
            "the JPEG file gives its height after its first scan (a DNL marker)",
        ));
    }
    if frame.width == 0 {
        return Err(malformed("the JPEG frame header gives a width of 0"));
    }
    let Some(components @ (1 | 3 | 4)) = u8::try_from(components).ok() else {
        return Err(unsupported(format!(
            "a JPEG file with {components} components cannot go into a PDF"
        )));
    };
    // libjpeg draws the only scan of a sequential file whatever follows it, as poppler
    // shows, and fails on what follows only when it reads on to the end of image, as
    // `qpdf --check` does: such a file goes in up to the end of that scan.
    let later = Later {
        frame: &frame,
        more_scans,
        interval,
        restarts,
    };
    let end = match check_later_scans(data, coded, later) {
        Ok(()) => None,
        Err(_) if !more_scans => Some(coded_data_end(data, coded, restarts)),
        Err(problem) => return Err(problem),
    };
    Ok((
        u32::from(frame.width),
        u32::from(frame.height),
        Encoding::Jpeg {
            components,
            adobe,
            end,
        },
    ))
}

/// Where the coded data of a scan that starts at `coded`, and has `restarts` restart
/// markers ([`scan_restarts`]), ends: at the first 0xFF, or run of them, before a marker
/// that ends the scan for libjpeg ([`at_restart`]), else at the end of `data`. Stuffed
/// zeros, restart markers, and the markers libjpeg skips at a restart marker it expects are
/// part of the coded data.
fn coded_data_end(data: &[u8], coded: usize, restarts: u64) -> usize {
    let mut restarts = Restarts::new(restarts);
    let mut at = coded;
    loop {
        while data.get(at).is_some_and(|&byte| byte != 0xFF) {
            at += 1;
        }
        let start = at;
        while data.get(at) == Some(&0xFF) {
            at += 1;
        }
        match data.get(at) {
            Some(0x00) => at += 1,
            Some(&code) if restarts.left > 0 => {
                match at_restart(data, start, code, at, &mut restarts) {
                    Resume::Data(next) => at = next,
                    Resume::Marker(start) => return start,
                }
            }
            Some(0xD0..=0xD7) => at += 1,
            _ => return start.min(data.len()),
        }
    }
}

/// How many restart markers libjpeg expects in the coded data of the scan whose header is
/// `segment`, after its length, in `frame` with a restart interval of `interval` MCUs: one
/// after every `interval` MCUs but the last, none without an interval (B.2.4.4, E.1.4). An
/// MCU is one block of the component of a scan of one component, and the blocks of every
/// component, in their sampling factors, of a scan of several (A.2).
fn scan_restarts(frame: &Frame, segment: &[u8], interval: u16) -> u64 {
    if interval == 0 {
        return 0;
    }
    let largest = |factor: fn(&(u8, u8, u8)) -> u8| {
        u64::from(
            frame
                .components
                .iter()
                .map(factor)
                .max()
                .unwrap_or(1)
                .max(1),
        )
    };
    let (h_max, v_max) = (largest(|c| c.1), largest(|c| c.2));
    let (width, height) = (u64::from(frame.width), u64::from(frame.height));
    let mcus = match segment {
        [1, id, ..] => {
            let Some(&(_, h, v)) = frame.components.iter().find(|c| c.0 == *id) else {
                return 0;
            };
            let columns = (width * u64::from(h)).div_ceil(h_max).div_ceil(8);
            let rows = (height * u64::from(v)).div_ceil(v_max).div_ceil(8);
            columns * rows
        }
        _ => width.div_ceil(8 * h_max) * height.div_ceil(8 * v_max),
    };
    mcus.div_ceil(u64::from(interval)).saturating_sub(1)
}

/// The restart markers libjpeg still expects in the coded data of a scan, and the number,
/// 0 to 7, of the next of them (B.2.4.4, E.1.4).
#[derive(Clone, Copy, Debug)]
struct Restarts {
    left: u64,
    next: u8,
}

impl Restarts {
    /// A scan's `left` restart markers, numbered from 0.
    fn new(left: u64) -> Self {
        Self { left, next: 0 }
    }

    /// One restart marker done: the next is expected.
    fn take(&mut self) {
        self.left = self.left.saturating_sub(1);
        self.next = (self.next + 1) % 8;
    }
}

/// Where the walk of a scan's coded data goes on after a marker met in it.
enum Resume {
    /// In the coded data, at this byte.
    Data(usize),
    /// The scan has ended: the walk takes the marker whose run of 0xFF starts at this byte,
    /// as it takes any marker between segments.
    Marker(usize),
}

/// The next marker of `data` from byte `from`, as libjpeg looks for it: past any bytes
/// before it, stuffed zeros among them, and the 0xFF bytes that pad it. Returns where its
/// run of 0xFF starts and its code's byte, or `None` at the end of `data`.
fn next_marker(data: &[u8], mut at: usize) -> Option<(usize, usize)> {
    loop {
        while data.get(at).is_some_and(|&byte| byte != 0xFF) {
            at += 1;
        }
        let start = at;
        while data.get(at) == Some(&0xFF) {
            at += 1;
        }
        match data.get(at)? {
            0x00 => at += 1,
            _ => return Some((start, at)),
        }
    }
}

/// What libjpeg does with a marker it meets in a scan's coded data while `restarts` are
/// still to come: the marker whose run of 0xFF starts at `start`, with `code` at byte `at`.
/// libjpeg takes any marker in coded data at the next restart point, where it expects
/// restart marker `restarts.next`, as `qpdf --check` shows on generated files:
///
/// - that restart marker is taken, and the coded data goes on after it;
/// - a restart marker one or two numbers further on is left unread, as if the expected
///   one were missing: the restart point passes, and the marker is met again at the next;
/// - a restart marker one or two numbers back is skipped, as is any marker of 0x01 to
///   0xBF, one libjpeg does not know among them: it warns and looks on for the next marker;
/// - any other restart marker is taken for the one expected;
/// - any other marker ends the scan: it is left unread at every restart point still to
///   come, and then read as any marker between segments is.
///
/// So a marker libjpeg does not know is skipped in the coded data while a restart marker
/// is still to come, and refused after the scan's last one.
fn at_restart(
    data: &[u8],
    mut start: usize,
    mut code: u8,
    mut at: usize,
    restarts: &mut Restarts,
) -> Resume {
    loop {
        if restarts.left == 0 {
            return Resume::Marker(start);
        }
        match code {
            0xD0..=0xD7 => match (code - 0xD0 + 8 - restarts.next) % 8 {
                0 | 3..=5 => {
                    restarts.take();
                    return Resume::Data(at + 1);
                }
                1 | 2 => {
                    restarts.take();
                    continue;
                }
                _ => {}
            },
            0x01..=0xBF => {}
            _ => {
                restarts.left = 0;
                return Resume::Marker(start);
            }
        }
        // Skipped: libjpeg looks on for the next marker.
        let Some((next_start, next_at)) = next_marker(data, at + 1) else {
            return Resume::Data(data.len());
        };
        let Some(&next_code) = data.get(next_at) else {
            return Resume::Data(data.len());
        };
        (start, code, at) = (next_start, next_code, next_at);
    }
}

/// Fails for a marker at byte `at` that libjpeg does not know, wherever it is in the file:
/// those T.81 reserves (0x02 to 0xBF, B.1.1.3) and JPG and JPGn (0xC8, 0xF0 to 0xFD), kept
/// for extensions. libjpeg stops at them ("Unsupported marker type", or "Unsupported JPEG
/// process: SOF type 0xc8"), so such a file draws nothing in a viewer that decodes with it.
fn refuse_unknown_marker(code: u8, at: usize) -> std::result::Result<(), Problem> {
    match code {
        0x02..=0xBF | 0xC8 | 0xF0..=0xFD => Err(malformed(format!(
            "the JPEG file has marker FF{code:02X} at byte {at}, which T.81 reserves"
        ))),
        _ => Ok(()),
    }
}

/// Fails for a marker of a frame that cannot go into a PDF: lossless, differential and
/// arithmetic-coded frames, and the DHP and EXP markers of hierarchical files (B.3,
/// Annex J). DHT, JPG and DAC are not frame headers.
fn refuse_other_frame(code: u8) -> std::result::Result<(), Problem> {
    match code {
        0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDE | 0xDF => Err(unsupported(
            "lossless, hierarchical and arithmetic-coded JPEG cannot go into a PDF",
        )),
        _ => Ok(()),
    }
}

/// Checks the scan headers after the first, whose coded data starts at `coded`, against
/// their `frame`, as libjpeg does when it reaches them, and refuses what libjpeg refuses
/// there too: a second frame header or start of image, the marker of a frame that cannot
/// go into a PDF ([`refuse_other_frame`]), and any scan at all unless `more_scans`. libjpeg
/// expects more scans after the first only in a progressive frame, or in a sequential one
/// whose first scan codes only some of its components; otherwise it stops at the next
/// scan header with "Didn't expect more than one scan". That rule matched libjpeg-turbo,
/// through `qpdf --check`, on all of 642 generated sequential files (`SOF0` and `SOF1`, 1,
/// 3 and 4 components, every first and second scan, and a third). Without `more_scans`,
/// [`read_jpeg`] cuts the file after its scan where this fails, rather than refusing it.
///
/// The coded data of a scan runs to the next marker, a 0xFF that is not a stuffed zero
/// (0xFF 0x00), fill (more 0xFF) or a restart marker (B.1.1.2, B.1.1.5). The check ends at
/// the end of image, after which libjpeg reads nothing; it also ends, refusing nothing,
/// where the file ends early. A marker libjpeg does not know is refused
/// ([`refuse_unknown_marker`]), but where libjpeg skips it: in the coded data of a scan
/// while a restart marker is still to come ([`at_restart`]).
fn check_later_scans(
    data: &[u8],
    coded: usize,
    later: Later<'_>,
) -> std::result::Result<(), Problem> {
    let Later {
        frame,
        more_scans,
        mut interval,
        restarts,
    } = later;
    let mut restarts = Restarts::new(restarts);
    let mut at = coded;
    loop {
        // Coded data, or bytes between segments, up to the next marker.
        let code = loop {
            while data.get(at).is_some_and(|&byte| byte != 0xFF) {
                at += 1;
            }
            let start = at;
            while data.get(at) == Some(&0xFF) {
                at += 1;
            }
            let Some(&code) = data.get(at) else {
                return Ok(());
            };
            match code {
                0x00 => at += 1,
                _ if restarts.left > 0 => match at_restart(data, start, code, at, &mut restarts) {
                    Resume::Data(next) => at = next,
                    // Met again with no restart marker to come.
                    Resume::Marker(start) => at = start,
                },
                // A restart marker the scan does not expect has no segment either.
                0xD0..=0xD7 => at += 1,
                _ => {
                    at += 1;
                    break code;
                }
            }
        };
        match code {
            0xD9 => return Ok(()),
            0xD8 => {
                return Err(malformed(format!(
                    "the JPEG file has a second start of image, at byte {}",
                    at - 2
                )));
            }
            // TEM has no segment.
            0x01 => continue,
            0xC0..=0xC2 => {
                return Err(malformed(format!(
                    "the JPEG file has a second frame header, at byte {}",
                    at - 2
                )));
            }
            // Segments, skipped by their length as before the first scan: 0xC3 to 0xCF
            // but JPG (the other frame markers, DHT and DAC), SOS, DQT, DNL, DRI, DHP, EXP,
            // APP0 to APP15 and COM.
            0xC3..=0xC7 | 0xC9..=0xCF | 0xDA..=0xDF | 0xE0..=0xEF | 0xFE => {}
            // Any other marker is one libjpeg does not know.
            _ => return refuse_unknown_marker(code, at - 2),
        }
        refuse_other_frame(code)?;
        let Some(length) = u16_at(data, at).map(usize::from) else {
            return Ok(());
        };
        let Some(segment) = data.get(at + 2..at + length.max(2)) else {
            return Ok(());
        };
        if code == 0xDD {
            interval = u16_at(segment, 0).unwrap_or(0);
        }
        // Only a scan header is followed by coded data, with restart markers in it.
        restarts = Restarts::new(0);
        if code == 0xDA {
            restarts = Restarts::new(scan_restarts(frame, segment, interval));
            // libjpeg reads the scan header before it finds the scan unexpected.
            check_scan(segment, length, frame)?;
            // `read_jpeg` cuts the file before such a scan rather than refusing it, so this
            // error, like any other here without `more_scans`, never reaches a caller.
            if !more_scans {
                return Err(malformed(format!(
                    "the JPEG file has a second scan at byte {}, after a first scan that codes \
                     every component of its sequential frame",
                    at - 2
                )));
            }
        }
        at += length.max(2);
    }
}

/// What [`check_later_scans`] reads after the first scan by: the `frame`, whether it takes
/// `more_scans`, the restart `interval` in effect, and how many `restarts` the first scan's
/// coded data holds ([`scan_restarts`]).
#[derive(Clone, Copy)]
struct Later<'a> {
    frame: &'a Frame,
    more_scans: bool,
    interval: u16,
    restarts: u64,
}

/// A JPEG frame header (ITU T.81, B.2.2).
struct Frame {
    precision: u8,
    height: u16,
    width: u16,
    /// Each component's identifier and horizontal and vertical sampling factors.
    components: Vec<(u8, u8, u8)>,
    /// Whether the frame is progressive (`SOF2`), whose scans each code some of the
    /// coefficients, to some bit (G.1.1.1).
    progressive: bool,
}

/// Reads a JPEG frame header, `segment` after its length, of a `progressive` frame or
/// not, and checks the rules of T.81 (B.2.2) that libjpeg enforces when it reads one:
/// 1 to 255 components of three bytes each, so that the header is `8 + 3 × components`
/// bytes long, with sampling factors of 1 to 4 and a quantization table of 0 to 3.
fn read_frame(segment: &[u8], progressive: bool) -> std::result::Result<Frame, Problem> {
    let Some((&[precision, h1, h0, w1, w0, count], specifications)) =
        segment.split_first_chunk::<6>()
    else {
        return Err(malformed("the JPEG frame header is truncated"));
    };
    if count == 0 {
        return Err(malformed("the JPEG frame header gives 0 components"));
    }
    let expected = 3 * usize::from(count);
    if specifications.len() != expected {
        return Err(malformed(format!(
            "the JPEG frame header is {} bytes long, not {} for {}",
            8 + specifications.len(),
            8 + expected,
            counted(count, "component")
        )));
    }
    let mut components = Vec::with_capacity(usize::from(count));
    for specification in specifications.chunks_exact(3) {
        let &[id, sampling, table] = specification else {
            continue;
        };
        let (horizontal, vertical) = (sampling >> 4, sampling & 0x0F);
        if !(1..=4).contains(&horizontal) || !(1..=4).contains(&vertical) {
            return Err(malformed(format!(
                "the JPEG frame header gives component {id} the sampling factors \
                 {horizontal} and {vertical}, not 1 to 4"
            )));
        }
        if table > 3 {
            return Err(malformed(format!(
                "the JPEG frame header gives component {id} quantization table {table}, \
                 not 0 to 3"
            )));
        }
        components.push((id, horizontal, vertical));
    }
    Ok(Frame {
        precision,
        height: u16::from_be_bytes([h1, h0]),
        width: u16::from_be_bytes([w1, w0]),
        components,
        progressive,
    })
}

/// Checks a JPEG scan header of `length` bytes, `segment` after the length, against its
/// `frame` by the rules of T.81 (B.2.3, G.1.1.1) that libjpeg enforces: 1 to 4
/// components, so that the header is `6 + 2 × components` bytes long, each a different
/// component of the frame, and in an interleaved scan at most 10 blocks in each minimum
/// coded unit. In a progressive frame the scan codes either the DC coefficients alone
/// (Ss = Se = 0) or AC coefficients of one component (1 <= Ss <= Se <= 63), refines by
/// one bit (Al = Ah - 1) unless it is the first scan of those coefficients (Ah = 0), and
/// has a point transform Al of at most 13. These rules matched libjpeg-turbo, through
/// `qpdf --check`, on all of 27,840 generated scans; it decodes a sequential frame's scan
/// whatever these four fields hold, with a warning, so they are not checked there.
///
/// Which component an identifier names follows libjpeg, which is looser than T.81. T.81
/// wants the frame's identifiers distinct and the scan in the frame's order; libjpeg
/// also decodes frames that repeat an identifier and some scans out of order, and takes
/// for the scan's component k (from 0) the frame's first component at position k or
/// later with its identifier. That rule matched libjpeg-turbo, through
/// `qpdf --check`, on all of 1394 generated scans.
fn check_scan(segment: &[u8], length: usize, frame: &Frame) -> std::result::Result<(), Problem> {
    let count = segment.first().copied().unwrap_or(0);
    if !(1..=4).contains(&count) {
        return Err(malformed(format!(
            "the JPEG scan header gives {count} components, not 1 to 4"
        )));
    }
    let expected = 6 + 2 * usize::from(count);
    if length != expected {
        return Err(malformed(format!(
            "the JPEG scan header is {length} bytes long, not {expected} for {}",
            counted(count, "component")
        )));
    }
    let ids: Vec<u8> = segment
        .get(1..)
        .unwrap_or_default()
        .chunks_exact(2)
        .filter_map(<[u8]>::first)
        .take(count.into())
        .copied()
        .collect();
    // The positions in the frame of the components named so far.
    let mut taken = Vec::with_capacity(ids.len());
    let mut blocks = 0;
    for (named, &id) in ids.iter().enumerate() {
        let found = frame
            .components
            .iter()
            .enumerate()
            .skip(named)
            .find(|(_, component)| component.0 == id)
            .filter(|(at, _)| !taken.contains(at));
        let Some((at, &(_, horizontal, vertical))) = found else {
            let in_frame = frame.components.iter().filter(|c| c.0 == id).count();
            let before = ids
                .get(..named)
                .unwrap_or_default()
                .iter()
                .filter(|&&x| x == id);
            let reason = if in_frame == 0 {
                ", which the frame header does not have"
            } else if before.count() >= in_frame {
                " more often than the frame header has it"
            } else {
                " out of the frame header's order"
            };
            return Err(malformed(format!(
                "the JPEG scan header names component {id}{reason}"
            )));
        };
        taken.push(at);
        blocks += u32::from(horizontal) * u32::from(vertical);
    }
    if count > 1 && blocks > 10 {
        return Err(malformed(format!(
            "the JPEG scan interleaves {blocks} blocks in each minimum coded unit, not at \
             most 10"
        )));
    }
    if !frame.progressive {
        return Ok(());
    }
    let Some(&[start, end, approximation]) = segment
        .get(1 + 2 * usize::from(count)..)
        .and_then(<[u8]>::first_chunk::<3>)
    else {
        return Err(malformed("the JPEG scan header is truncated"));
    };
    let (high, low) = (approximation >> 4, approximation & 0x0F);
    if !(start == 0 && end == 0 || (1..=end).contains(&start) && end <= 63) {
        return Err(malformed(format!(
            "the progressive JPEG scan header gives Ss {start} and Se {end}, not 0 and 0 \
             for the DC coefficients or 1 <= Ss <= Se <= 63 for AC coefficients"
        )));
    }
    if start > 0 && count > 1 {
        return Err(malformed(format!(
            "the progressive JPEG scan header names {} in a scan of AC coefficients, not 1",
            counted(count, "component")
        )));
    }
    if high != 0 && low + 1 != high {
        return Err(malformed(format!(
            "the progressive JPEG scan header gives Ah {high} and Al {low}, not \
             Al = Ah - 1 for a refinement"
        )));
    }
    if low > 13 {
        return Err(malformed(format!(
            "the progressive JPEG scan header gives Al {low}, not 0 to 13"
        )));
    }
    Ok(())
}

/// `count` and `noun`, in the plural unless `count` is 1: "1 component", "3 components".
fn counted(count: u8, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The header of a PNG file (PNG specification, 11.2.2).
#[derive(Clone, Copy, Debug)]
struct Header {
    width: u32,
    height: u32,
    bits: u8,
    color_type: u8,
    interlaced: bool,
}

impl Header {
    /// The samples per pixel.
    fn channels(self) -> u64 {
        match self.color_type {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }

    /// The bytes of a row of `width` pixels, without its filter byte.
    fn row_bytes(self, width: u64) -> u64 {
        (width * self.channels() * u64::from(self.bits)).div_ceil(8)
    }

    /// The width and height of each image the data holds: the whole image, or the seven
    /// passes of Adam7 interlacing (PNG specification, 8.2), each with where its first
    /// pixel is and how far apart its pixels are.
    fn passes(self) -> Vec<Pass> {
        let (width, height) = (u64::from(self.width), u64::from(self.height));
        if !self.interlaced {
            return vec![Pass {
                x: 0,
                y: 0,
                dx: 1,
                dy: 1,
                width,
                height,
            }];
        }
        [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
        .into_iter()
        .map(|(x, y, dx, dy)| Pass {
            x,
            y,
            dx,
            dy,
            width: width.saturating_sub(x).div_ceil(dx),
            height: height.saturating_sub(y).div_ceil(dy),
        })
        .collect()
    }

    /// The size of the decompressed data: every row of every pass with its filter byte. A
    /// pass with no pixels has no rows. `None` if it does not fit in 64 bits, as for a
    /// 16-bit colour image of 2^31 - 1 by 2^31 - 1 pixels.
    fn data_size(self) -> Option<u64> {
        self.passes()
            .iter()
            .filter(|pass| pass.width > 0)
            .try_fold(0u64, |size, pass| {
                let row = self.row_bytes(pass.width).checked_add(1)?;
                size.checked_add(pass.height.checked_mul(row)?)
            })
    }
}

#[derive(Clone, Copy, Debug)]
struct Pass {
    x: u64,
    y: u64,
    dx: u64,
    dy: u64,
    width: u64,
    height: u64,
}

/// Reads a PNG file (PNG specification, third edition): its header, palette, transparency
/// and compressed data, checking the CRC of every critical chunk and that the data
/// decompresses to the size the header gives.
fn read_png(data: &[u8]) -> std::result::Result<(u32, u32, Encoding), Problem> {
    let mut at = PNG_SIGNATURE.len();
    let mut header = None;
    let mut palette = None;
    let mut transparency = None;
    let mut compressed = Vec::new();
    // Where the data of each chunk is, until those places would take more than a quarter
    // of the data: the copy, which decompressing needs anyway, is kept then.
    let mut chunks = Some(Vec::new());
    loop {
        let length = u32_at(data, at).ok_or_else(|| malformed("the PNG file has no IEND chunk"))?;
        // A chunk's length is at most 2^31 - 1 (5.3).
        let length = usize::try_from(length)
            .ok()
            .filter(|&length| length < 1 << 31)
            .ok_or_else(|| malformed(format!("the PNG chunk at byte {at} is too long")))?;
        let kind = data
            .get(at + 4..at + 8)
            .ok_or_else(|| malformed("the PNG file has no IEND chunk"))?;
        let end = at.saturating_add(8).saturating_add(length);
        let (Some(body), Some(crc)) = (data.get(at + 8..end), u32_at(data, end)) else {
            return Err(malformed(format!(
                "the PNG chunk at byte {at} is truncated"
            )));
        };
        let critical = kind.first().is_some_and(u8::is_ascii_uppercase);
        let damaged = crc32(data.get(at + 4..end).unwrap_or_default()) != crc;
        if damaged && critical {
            return Err(malformed(format!(
                "the PNG chunk {} at byte {at} has a wrong CRC",
                String::from_utf8_lossy(kind)
            )));
        }
        at = end.saturating_add(4);
        // An ancillary chunk with a wrong CRC is left out, as libpng leaves it out.
        if damaged {
            continue;
        }
        if header.is_none() && kind != b"IHDR" {
            return Err(malformed("the PNG file does not start with an IHDR chunk"));
        }
        match kind {
            b"IHDR" => header = Some(png_header(body)?),
            b"PLTE" => palette = Some(body.to_vec()),
            b"tRNS" => transparency = Some(body.to_vec()),
            b"IDAT" => {
                compressed.extend_from_slice(body);
                if let Some(places) = &mut chunks {
                    if !body.is_empty() {
                        places.push(end - body.len()..end);
                    }
                    if 4 * size_of::<Range<usize>>() * places.len() > compressed.len() {
                        chunks = None;
                    }
                }
            }
            b"IEND" => break,
            // A critical chunk this reader does not know (bit 5 of the first byte clear).
            [first, ..] if first.is_ascii_uppercase() => {
                return Err(unsupported(format!(
                    "the PNG file has a critical chunk {} this reader does not know",
                    String::from_utf8_lossy(kind)
                )));
            }
            _ => {}
        }
    }
    let Some(header) = header else {
        return Err(malformed("the PNG file has no IHDR chunk"));
    };
    let size = header
        .data_size()
        .filter(|&size| size <= MAX_PNG_DATA)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or(Problem::TooLarge)?;

    let palette = match (header.color_type, palette) {
        (3, None) => return Err(malformed("the PNG palette image has no PLTE chunk")),
        (3, Some(palette))
            if palette.is_empty() || palette.len() % 3 != 0 || palette.len() > 768 =>
        {
            return Err(malformed(format!(
                "the PNG palette has {} bytes, not 3 for each of 1 to 256 entries",
                palette.len()
            )));
        }
        (3, Some(palette)) => Some(palette),
        // A palette suggested for a true colour image, or a forbidden one; neither
        // changes the image.
        _ => None,
    };
    // Transparency is part of the pixels for images with an alpha channel, and a tRNS
    // chunk there is not allowed (11.3.2.1); it is ignored, as decoders do. So is a tRNS
    // chunk of the wrong size for a greyscale or true colour image, and a palette's is
    // cut to the palette's entries.
    let transparency = match (header.color_type, transparency, &palette) {
        (0, Some(key), _) if key.len() == 2 => Some(key),
        (2, Some(key), _) if key.len() == 6 => Some(key),
        (3, Some(mut alpha), Some(palette)) => {
            // Only the entries that exist and that the bit depth can address.
            alpha.truncate((palette.len() / 3).min(1 << header.bits));
            Some(alpha)
        }
        _ => None,
    };

    let space = match (header.color_type, &palette) {
        (0 | 4, _) => ColorSpace::Gray,
        (3, Some(palette)) => {
            // As many entries as the bit depth addresses, so every index has a colour;
            // indices beyond the file's palette are black.
            let mut padded = palette.clone();
            padded.resize(3usize << header.bits, 0);
            ColorSpace::Indexed(padded)
        }
        _ => ColorSpace::Rgb,
    };
    // Passed through: images without interlacing or alpha, and palette images whose
    // transparency a colour key expresses. A transparent colour of a greyscale or true
    // colour image is decoded too.
    let pass_through = match (header.interlaced, header.color_type, &transparency) {
        (false, 0 | 2 | 3, None) => Some(None),
        (false, 3, Some(alpha)) => palette_mask(alpha),
        _ => None,
    };
    if pass_through.is_none() {
        // The decoded samples and alpha: as many as 8 + 8 bytes for each pixel of 1 bit.
        let pixels = u64::from(header.width) * u64::from(header.height);
        let color = if matches!(header.color_type, 0 | 4) {
            1
        } else {
            3
        };
        let alpha = u64::from(matches!(header.color_type, 4 | 6) || transparency.is_some());
        if pixels.saturating_mul(color + alpha) > MAX_PNG_DATA {
            return Err(Problem::TooLarge);
        }
    }
    let rows = inflate(&compressed, size)?;
    check_filters(header, &rows)?;
    if let Some(mask) = pass_through {
        return Ok((
            header.width,
            header.height,
            Encoding::Png {
                data: match chunks {
                    Some(mut places) => {
                        places.shrink_to_fit();
                        PngData::InFile(places)
                    }
                    None => PngData::Copied(compressed),
                },
                space,
                bits: header.bits,
                mask,
            },
        ));
    }
    let (samples, alpha) = decode(header, &rows, palette.as_deref(), transparency.as_deref());
    let space = match space {
        ColorSpace::Indexed(_) => ColorSpace::Rgb,
        space => space,
    };
    let alpha = alpha.filter(|alpha| alpha.iter().any(|&a| a != 255));
    Ok((
        header.width,
        header.height,
        Encoding::Decoded {
            samples,
            space,
            alpha,
        },
    ))
}

/// The header's fields, checked (PNG specification, 11.2.2).
fn png_header(body: &[u8]) -> std::result::Result<Header, Problem> {
    let (Some(width), Some(height), Some(&[bits, color_type, compression, filter, interlace])) = (
        u32_at(body, 0),
        u32_at(body, 4),
        body.get(8..13)
            .and_then(|rest| <&[u8; 5]>::try_from(rest).ok()),
    ) else {
        return Err(malformed("the PNG IHDR chunk is not 13 bytes long"));
    };
    if body.len() != 13 {
        return Err(malformed("the PNG IHDR chunk is not 13 bytes long"));
    }
    let limit = 1 << 31;
    if width == 0 || height == 0 || width >= limit || height >= limit {
        return Err(malformed(format!(
            "the PNG image is {width} x {height} pixels; each side must be 1 to 2^31 - 1"
        )));
    }
    let allowed: &[u8] = match color_type {
        0 => &[1, 2, 4, 8, 16],
        3 => &[1, 2, 4, 8],
        2 | 4 | 6 => &[8, 16],
        _ => {
            return Err(malformed(format!(
                "the PNG colour type {color_type} does not exist"
            )));
        }
    };
    if !allowed.contains(&bits) {
        return Err(malformed(format!(
            "the PNG bit depth {bits} is not allowed for colour type {color_type}"
        )));
    }
    if compression != 0 || filter != 0 {
        return Err(unsupported(format!(
            "the PNG compression method {compression} or filter method {filter} is not the \
             standard one, 0"
        )));
    }
    if interlace > 1 {
        return Err(malformed(format!(
            "the PNG interlace method {interlace} does not exist"
        )));
    }
    Ok(Header {
        width,
        height,
        bits,
        color_type,
        interlaced: interlace == 1,
    })
}

/// The colour-key mask of palette transparency, if PDF can express it: the fully
/// transparent entries form one range of indices and every other entry is opaque
/// (ISO 32000-1, 8.9.6.4). `Some(None)` if every entry is opaque.
fn palette_mask(alpha: &[u8]) -> Option<Option<(u8, u8)>> {
    if alpha.iter().any(|&a| a != 0 && a != 255) {
        return None;
    }
    let first = alpha.iter().position(|&a| a == 0);
    let last = alpha.iter().rposition(|&a| a == 0);
    match (first, last) {
        (Some(first), Some(last)) => {
            let range = alpha.get(first..=last).unwrap_or_default();
            if range.iter().all(|&a| a == 0) {
                Some(Some((u8::try_from(first).ok()?, u8::try_from(last).ok()?)))
            } else {
                None
            }
        }
        _ => Some(None),
    }
}

/// Decompresses the zlib stream `compressed` and checks that it holds at least `size`
/// bytes; data after them is ignored, as decoders ignore it.
fn inflate(compressed: &[u8], size: usize) -> std::result::Result<Vec<u8>, Problem> {
    match decompress_to_vec_zlib_with_limit(compressed, size) {
        Ok(data) if data.len() == size => Ok(data),
        Ok(data) => Err(malformed(format!(
            "the PNG image data decompresses to {} bytes, not {size}",
            data.len()
        ))),
        // The output is full: the stream holds more than `size` bytes.
        Err(err) if err.status == TINFLStatus::HasMoreOutput => {
            let mut data = err.output;
            data.truncate(size);
            Ok(data)
        }
        // On other errors the output is not cut to what was decompressed.
        Err(err) => Err(malformed(format!("the PNG image data is damaged: {err}"))),
    }
}

/// Fails unless every row of every pass starts with one of the five filter types.
fn check_filters(header: Header, rows: &[u8]) -> std::result::Result<(), Problem> {
    let mut at = 0;
    for pass in header.passes().iter().filter(|pass| pass.width > 0) {
        let length = usize::try_from(1 + header.row_bytes(pass.width)).unwrap_or(usize::MAX);
        for _ in 0..pass.height {
            match rows.get(at) {
                Some(0..=4) => at += length,
                Some(filter) => {
                    return Err(malformed(format!(
                        "the PNG row at byte {at} of the image data has filter type {filter}"
                    )));
                }
                None => return Err(malformed("the PNG image data is truncated")),
            }
        }
    }
    Ok(())
}

/// Decodes the filtered `rows` of a checked image into 8-bit greyscale or RGB samples and
/// an alpha value per pixel, `None` for an image that cannot be transparent.
fn decode(
    header: Header,
    rows: &[u8],
    palette: Option<&[u8]>,
    transparency: Option<&[u8]>,
) -> (Vec<u8>, Option<Vec<u8>>) {
    let color = match header.color_type {
        0 | 4 => 1,
        _ => 3,
    };
    let has_alpha = matches!(header.color_type, 4 | 6) || transparency.is_some();
    let pixels =
        usize::try_from(u64::from(header.width) * u64::from(header.height)).unwrap_or(usize::MAX);
    let mut samples = vec![0; pixels.saturating_mul(color)];
    let mut alpha = vec![255; if has_alpha { pixels } else { 0 }];
    let channels = usize::try_from(header.channels()).unwrap_or(1);
    let bytes_per_pixel = (channels * usize::from(header.bits)).div_ceil(8);
    let width = usize::try_from(header.width).unwrap_or(usize::MAX);

    let mut at = 0;
    for pass in header.passes().iter().filter(|pass| pass.width > 0) {
        let length = usize::try_from(header.row_bytes(pass.width)).unwrap_or(usize::MAX);
        let mut previous = vec![0; length];
        let mut row = vec![0; length];
        for j in 0..pass.height {
            let filter = rows.get(at).copied().unwrap_or(0);
            row.copy_from_slice(rows.get(at + 1..at + 1 + length).unwrap_or(&previous));
            at += 1 + length;
            unfilter(filter, &mut row, &previous, bytes_per_pixel);
            let y = usize::try_from(pass.y + j * pass.dy).unwrap_or(usize::MAX);
            for i in 0..pass.width {
                let x = usize::try_from(pass.x + i * pass.dx).unwrap_or(usize::MAX);
                let pixel = y.saturating_mul(width).saturating_add(x);
                let i = usize::try_from(i).unwrap_or(usize::MAX);
                let (rgb, a) = pixel_at(header, &row, i, palette, transparency);
                if let Some(out) = samples.get_mut(pixel * color..(pixel + 1) * color) {
                    out.copy_from_slice(rgb.get(..color).unwrap_or_default());
                }
                if let Some(out) = alpha.get_mut(pixel) {
                    *out = a;
                }
            }
            std::mem::swap(&mut previous, &mut row);
        }
    }
    (samples, has_alpha.then_some(alpha))
}

/// Undoes a row's filter (PNG specification, 7.3 and 9), given the row above it.
fn unfilter(filter: u8, row: &mut [u8], previous: &[u8], bytes_per_pixel: usize) {
    for i in 0..row.len() {
        let left = i
            .checked_sub(bytes_per_pixel)
            .and_then(|j| row.get(j))
            .copied()
            .unwrap_or(0);
        let up = previous.get(i).copied().unwrap_or(0);
        let up_left = i
            .checked_sub(bytes_per_pixel)
            .and_then(|j| previous.get(j))
            .copied()
            .unwrap_or(0);
        let predictor = match filter {
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth(left, up, up_left),
            _ => 0,
        };
        if let Some(byte) = row.get_mut(i) {
            *byte = byte.wrapping_add(predictor);
        }
    }
}

/// The Paeth predictor (PNG specification, 9.4).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// The colour, as up to three 8-bit samples, and the alpha of pixel `i` of an unfiltered
/// row.
fn pixel_at(
    header: Header,
    row: &[u8],
    i: usize,
    palette: Option<&[u8]>,
    transparency: Option<&[u8]>,
) -> ([u8; 3], u8) {
    let bits = usize::from(header.bits);
    let channels = usize::try_from(header.channels()).unwrap_or(1);
    // Sample `c` at the image's bit depth.
    let sample = |c: usize| -> u16 {
        let index = i * channels + c;
        match bits {
            16 => u16_at(row, 2 * index).unwrap_or(0),
            8 => row.get(index).copied().map_or(0, u16::from),
            _ => {
                let bit = index * bits;
                let byte = row.get(bit / 8).copied().unwrap_or(0);
                let shift = 8 - bits - bit % 8;
                u16::from(byte >> shift) & ((1 << bits) - 1)
            }
        }
    };
    // A sample scaled to 8 bits: the high byte of a 16-bit one, a smaller one stretched
    // so that its largest value is 255.
    let eight = |value: u16| -> u8 {
        match bits {
            16 => (value >> 8) as u8,
            8 => value as u8,
            _ => (u32::from(value) * 255 / ((1 << bits) - 1)) as u8,
        }
    };
    // A colour is transparent if its samples equal those of the tRNS chunk, compared at
    // the image's bit depth (11.3.2.1).
    let keyed = |values: &[u16]| -> u8 {
        let key: Vec<u16> = (0..values.len())
            .map(|c| {
                transparency
                    .and_then(|t| u16_at(t, 2 * c))
                    .unwrap_or(u16::MAX)
            })
            .collect();
        if transparency.is_some() && key == values {
            0
        } else {
            255
        }
    };
    match header.color_type {
        0 => {
            let gray = sample(0);
            ([eight(gray), 0, 0], keyed(&[gray]))
        }
        2 => {
            let rgb = [sample(0), sample(1), sample(2)];
            ([eight(rgb[0]), eight(rgb[1]), eight(rgb[2])], keyed(&rgb))
        }
        3 => {
            let index = usize::from(sample(0));
            let entry = palette
                .and_then(|palette| palette.get(3 * index..3 * index + 3))
                .unwrap_or(&[0, 0, 0]);
            let alpha = transparency
                .and_then(|alpha| alpha.get(index))
                .copied()
                .unwrap_or(255);
            (
                [
                    entry.first().copied().unwrap_or(0),
                    entry.get(1).copied().unwrap_or(0),
                    entry.get(2).copied().unwrap_or(0),
                ],
                alpha,
            )
        }
        4 => ([eight(sample(0)), 0, 0], eight(sample(1))),
        _ => (
            [eight(sample(0)), eight(sample(1)), eight(sample(2))],
            eight(sample(3)),
        ),
    }
}

/// The CRC-32 of PNG chunks (PNG specification, annex D).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
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

/// The operators that draw an image resource `/Im<resource>` into the rectangle with its
/// lower-left corner at (`x`, `y`): an image fills the unit square of its space.
pub(crate) fn draw_operators(resource: usize, x: f64, y: f64, width: f64, height: f64) -> String {
    format!(
        "q\n{} 0 0 {} {} {} cm\n/Im{resource} Do\nQ\n",
        number(width),
        number(height),
        number(x),
        number(y)
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use miniz_oxide::deflate::compress_to_vec_zlib;

    use super::*;

    /// A PNG file made from pixels, to test the reader with: every colour type, bit depth,
    /// filter and interlacing.
    #[derive(Clone)]
    pub(crate) struct Png {
        width: u32,
        height: u32,
        color_type: u8,
        bits: u8,
        pub(crate) interlaced: bool,
        /// The samples of each pixel, row by row, at the bit depth.
        pixels: Vec<Vec<u16>>,
        palette: Option<Vec<u8>>,
        transparency: Option<Vec<u8>>,
        /// The filter type of the rows; 5 cycles through all five.
        filter: u8,
    }

    impl Png {
        /// An image of pseudo-random samples.
        pub(crate) fn new(width: u32, height: u32, color_type: u8, bits: u8) -> Self {
            let channels = match color_type {
                2 => 3,
                4 => 2,
                6 => 4,
                _ => 1,
            };
            let mut seed: u32 = width * 31 + height * 7 + u32::from(color_type) + u32::from(bits);
            let mut next = || {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 8) as u16
            };
            let max = if bits == 16 {
                u16::MAX
            } else {
                (1 << bits) - 1
            };
            let pixels = (0..width * height)
                .map(|_| (0..channels).map(|_| next() & max).collect())
                .collect();
            Self {
                width,
                height,
                color_type,
                bits,
                interlaced: false,
                pixels,
                palette: (color_type == 3)
                    .then(|| (0..=255).flat_map(|i| [i, 255 - i, i / 2]).collect()),
                transparency: None,
                filter: 5,
            }
        }

        fn row(&self, pixels: &[Vec<u16>]) -> Vec<u8> {
            let bits = usize::from(self.bits);
            let samples: Vec<u16> = pixels.iter().flatten().copied().collect();
            let mut row = vec![0u8; (samples.len() * bits).div_ceil(8)];
            for (index, &value) in samples.iter().enumerate() {
                match bits {
                    16 => row[2 * index..2 * index + 2].copy_from_slice(&value.to_be_bytes()),
                    8 => row[index] = value as u8,
                    _ => {
                        let bit = index * bits;
                        row[bit / 8] |= (value as u8) << (8 - bits - bit % 8);
                    }
                }
            }
            row
        }

        /// The filtered rows of the image or of each Adam7 pass, written here from the
        /// PNG specification rather than with the reader's code.
        fn rows(&self) -> Vec<u8> {
            let bpp = (self.pixels[0].len() * usize::from(self.bits)).div_ceil(8);
            // Where each pass starts and how far apart its pixels are (8.2).
            let passes: &[(u32, u32, u32, u32)] = if self.interlaced {
                &[
                    (0, 0, 8, 8),
                    (4, 0, 8, 8),
                    (0, 4, 4, 8),
                    (2, 0, 4, 4),
                    (0, 2, 2, 4),
                    (1, 0, 2, 2),
                    (0, 1, 1, 2),
                ]
            } else {
                &[(0, 0, 1, 1)]
            };
            let mut out = Vec::new();
            let mut count = 0u8;
            for &(x0, y0, dx, dy) in passes {
                let xs: Vec<u32> = (x0..self.width).step_by(dx as usize).collect();
                if xs.is_empty() {
                    continue;
                }
                let mut previous: Vec<u8> = Vec::new();
                for y in (y0..self.height).step_by(dy as usize) {
                    let pixels: Vec<Vec<u16>> = xs
                        .iter()
                        .map(|&x| self.pixels[(y * self.width + x) as usize].clone())
                        .collect();
                    let raw = self.row(&pixels);
                    let filter = if self.filter == 5 {
                        count % 5
                    } else {
                        self.filter
                    };
                    count = count.wrapping_add(1);
                    out.push(filter);
                    for i in 0..raw.len() {
                        let left = if i >= bpp { raw[i - bpp] } else { 0 };
                        let up = previous.get(i).copied().unwrap_or(0);
                        let up_left = if i >= bpp {
                            previous.get(i - bpp).copied().unwrap_or(0)
                        } else {
                            0
                        };
                        let predictor = match filter {
                            1 => left,
                            2 => up,
                            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                            4 => {
                                // The Paeth predictor as the specification gives it (9.4).
                                let (a, b, c) =
                                    (i16::from(left), i16::from(up), i16::from(up_left));
                                let p = a + b - c;
                                let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                                if pa <= pb && pa <= pc {
                                    left
                                } else if pb <= pc {
                                    up
                                } else {
                                    up_left
                                }
                            }
                            _ => 0,
                        };
                        out.push(raw[i].wrapping_sub(predictor));
                    }
                    previous = raw;
                }
            }
            out
        }

        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            let crc = crc32(&[&kind[..], body].concat());
            out.extend_from_slice(&crc.to_be_bytes());
        }

        fn header(&self) -> Vec<u8> {
            let mut body = Vec::new();
            body.extend_from_slice(&self.width.to_be_bytes());
            body.extend_from_slice(&self.height.to_be_bytes());
            body.extend_from_slice(&[self.bits, self.color_type, 0, 0, u8::from(self.interlaced)]);
            body
        }

        fn encode_with(&self, data: &[u8]) -> Vec<u8> {
            let mut out = PNG_SIGNATURE.to_vec();
            Self::chunk(&mut out, b"IHDR", &self.header());
            if let Some(palette) = &self.palette {
                Self::chunk(&mut out, b"PLTE", palette);
            }
            if let Some(transparency) = &self.transparency {
                Self::chunk(&mut out, b"tRNS", transparency);
            }
            // Two IDAT chunks, which the reader joins.
            let (first, second) = data.split_at(data.len() / 2);
            Self::chunk(&mut out, b"IDAT", first);
            Self::chunk(&mut out, b"tEXt", b"Comment\0between data chunks");
            Self::chunk(&mut out, b"IDAT", second);
            Self::chunk(&mut out, b"IEND", b"");
            out
        }

        pub(crate) fn encode(&self) -> Vec<u8> {
            self.encode_with(&compress_to_vec_zlib(&self.rows(), 6))
        }

        /// The 8-bit samples and alpha the image decodes to, by the PNG specification.
        fn expected(&self) -> (Vec<u8>, Vec<u8>) {
            let eight = |v: u16| match self.bits {
                16 => (v >> 8) as u8,
                8 => v as u8,
                b => (u32::from(v) * 255 / ((1 << b) - 1)) as u8,
            };
            let mut samples = Vec::new();
            let mut alpha = Vec::new();
            for pixel in &self.pixels {
                match self.color_type {
                    3 => {
                        let i = usize::from(pixel[0]);
                        let palette = self.palette.as_deref().unwrap();
                        samples.extend(palette.get(3 * i..3 * i + 3).unwrap_or(&[0, 0, 0]));
                        let t = self.transparency.as_deref().unwrap_or(&[]);
                        alpha.push(t.get(i).copied().unwrap_or(255));
                    }
                    0 | 2 => {
                        samples.extend(pixel.iter().map(|&v| eight(v)));
                        let key: Option<Vec<u16>> = self.transparency.as_ref().map(|t| {
                            t.chunks(2)
                                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                                .collect()
                        });
                        alpha.push(if key.as_ref() == Some(pixel) { 0 } else { 255 });
                    }
                    _ => {
                        let (color, a) = pixel.split_at(pixel.len() - 1);
                        samples.extend(color.iter().map(|&v| eight(v)));
                        alpha.push(eight(a[0]));
                    }
                }
            }
            (samples, alpha)
        }
    }

    fn decoded(image: &Image) -> (&[u8], &ColorSpace, Option<&[u8]>) {
        match &image.inner.encoding {
            Encoding::Decoded {
                samples,
                space,
                alpha,
            } => (samples, space, alpha.as_deref()),
            other => panic!("not decoded: {other:?}"),
        }
    }

    const KINDS: [(u8, &[u8]); 5] = [
        (0, &[1, 2, 4, 8, 16]),
        (2, &[8, 16]),
        (3, &[1, 2, 4, 8]),
        (4, &[8, 16]),
        (6, &[8, 16]),
    ];

    const SIZES: [(u32, u32); 6] = [(1, 1), (3, 5), (8, 8), (9, 9), (17, 3), (2, 33)];

    #[test]
    fn passes_plain_png_files_through() {
        for (color_type, depths) in KINDS.iter().filter(|(t, _)| matches!(t, 0 | 2 | 3)) {
            for &bits in *depths {
                for (width, height) in SIZES {
                    let png = Png::new(width, height, *color_type, bits);
                    let image = Image::from_bytes(png.encode()).unwrap();
                    assert_eq!((image.width(), image.height()), (width, height));
                    let Encoding::Png {
                        data,
                        space,
                        bits: written,
                        mask,
                    } = &image.inner.encoding
                    else {
                        panic!("type {color_type} at {bits} bits was not passed through");
                    };
                    assert_eq!(
                        *image.inner.png_data(data),
                        compress_to_vec_zlib(&png.rows(), 6)
                    );
                    assert_eq!((*written, *mask), (bits, None));
                    assert_eq!(image.needs_pdf_15(), bits == 16);
                    match space {
                        ColorSpace::Indexed(palette) => {
                            // Every index the bit depth can address has a colour.
                            assert_eq!(palette.len(), 3 << bits);
                            assert_eq!(&palette[..], &png.palette.as_ref().unwrap()[..3 << bits]);
                        }
                        space => {
                            assert_eq!(
                                *space,
                                if *color_type == 0 {
                                    ColorSpace::Gray
                                } else {
                                    ColorSpace::Rgb
                                }
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn decodes_every_kind_of_png_as_the_specification_says() {
        for (color_type, depths) in KINDS {
            for &bits in depths {
                for (width, height) in SIZES {
                    for interlaced in [false, true] {
                        let mut png = Png::new(width, height, color_type, bits);
                        png.interlaced = interlaced;
                        if !interlaced && matches!(color_type, 0 | 2 | 3) {
                            // Passed through; transparency makes them decoded.
                            png.transparency = Some(match color_type {
                                0 => png.pixels[0].iter().flat_map(|v| v.to_be_bytes()).collect(),
                                2 => png.pixels[0].iter().flat_map(|v| v.to_be_bytes()).collect(),
                                _ => vec![0, 128, 0, 255, 0],
                            });
                        }
                        let image = Image::from_bytes(png.encode()).unwrap();
                        let (samples, space, alpha) = decoded(&image);
                        let (expected, expected_alpha) = png.expected();
                        let case = format!(
                            "type {color_type}, {bits} bits, {width} x {height}, interlaced {interlaced}"
                        );
                        assert_eq!(samples, expected, "{case}");
                        assert_eq!(
                            *space,
                            if matches!(color_type, 0 | 4) {
                                ColorSpace::Gray
                            } else {
                                ColorSpace::Rgb
                            },
                            "{case}"
                        );
                        let transparent = expected_alpha.iter().any(|&a| a != 255);
                        assert_eq!(alpha, transparent.then_some(&expected_alpha[..]), "{case}");
                        assert!(!image.needs_pdf_15());
                    }
                }
            }
        }
    }

    #[test]
    fn decodes_each_filter_type_alone() {
        for filter in 0..=4 {
            let mut png = Png::new(9, 7, 6, 8);
            png.filter = filter;
            let image = Image::from_bytes(png.encode()).unwrap();
            let (samples, _, alpha) = decoded(&image);
            let (expected, expected_alpha) = png.expected();
            assert_eq!(
                (samples, alpha),
                (&expected[..], Some(&expected_alpha[..])),
                "filter {filter}"
            );
        }
    }

    #[test]
    fn scales_small_samples_to_eight_bits() {
        let mut png = Png::new(4, 1, 0, 2);
        png.pixels = vec![vec![0], vec![1], vec![2], vec![3]];
        png.interlaced = true;
        assert_eq!(
            decoded(&Image::from_bytes(png.encode()).unwrap()).0,
            [0, 85, 170, 255]
        );
        let mut png = Png::new(2, 1, 0, 4);
        png.pixels = vec![vec![7], vec![15]];
        png.interlaced = true;
        assert_eq!(
            decoded(&Image::from_bytes(png.encode()).unwrap()).0,
            [119, 255]
        );
        let mut png = Png::new(2, 1, 2, 16);
        png.pixels = vec![vec![0x12FF, 0, 0xFFFF], vec![0x00FF, 0x8000, 1]];
        png.interlaced = true;
        assert_eq!(
            decoded(&Image::from_bytes(png.encode()).unwrap()).0,
            [0x12, 0, 0xFF, 0, 0x80, 0]
        );
    }

    #[test]
    fn palette_transparency_becomes_a_colour_key_mask_when_it_can() {
        let png = |transparency: &[u8]| {
            let mut png = Png::new(5, 4, 3, 4);
            png.transparency = Some(transparency.to_vec());
            Image::from_bytes(png.encode()).unwrap()
        };
        let mask = |image: &Image| match &image.inner.encoding {
            Encoding::Png { mask, .. } => Some(*mask),
            _ => None,
        };
        // Fully transparent entries in one range, the others opaque.
        assert_eq!(mask(&png(&[255, 0, 0, 0, 255])), Some(Some((1, 3))));
        assert_eq!(mask(&png(&[0])), Some(Some((0, 0))));
        assert_eq!(mask(&png(&[255, 255, 0])), Some(Some((2, 2))));
        // No transparent entry.
        assert_eq!(mask(&png(&[255, 255])), Some(None));
        assert_eq!(mask(&png(&[])), Some(None));
        // Two ranges, or partial transparency: decoded, with a soft mask.
        for transparency in [&[0, 255, 0][..], &[255, 128], &[0, 1]] {
            let image = png(transparency);
            assert_eq!(mask(&image), None, "{transparency:?}");
            assert!(decoded(&image).2.is_some());
        }
    }

    #[test]
    fn opaque_alpha_needs_no_soft_mask() {
        let mut png = Png::new(3, 3, 6, 8);
        for pixel in &mut png.pixels {
            pixel[3] = 255;
        }
        assert_eq!(decoded(&Image::from_bytes(png.encode()).unwrap()).2, None);
        let mut png = Png::new(3, 3, 4, 16);
        for pixel in &mut png.pixels {
            pixel[1] = 0xFFFF;
        }
        assert_eq!(decoded(&Image::from_bytes(png.encode()).unwrap()).2, None);
    }

    #[test]
    fn keys_transparent_colours_at_the_full_bit_depth() {
        let mut png = Png::new(2, 1, 2, 16);
        png.pixels = vec![vec![0x1234, 5, 6], vec![0x1235, 5, 6]];
        png.transparency = Some(vec![0x12, 0x34, 0, 5, 0, 6]);
        let image = Image::from_bytes(png.encode()).unwrap();
        // The high bytes are equal, but only the first pixel is the key.
        assert_eq!(decoded(&image).2, Some(&[0, 255][..]));
    }

    #[test]
    fn indices_beyond_the_palette_are_black() {
        let mut png = Png::new(3, 1, 3, 2);
        png.palette = Some(vec![10, 20, 30]);
        png.pixels = vec![vec![0], vec![1], vec![3]];
        let image = Image::from_bytes(png.encode()).unwrap();
        let Encoding::Png {
            space: ColorSpace::Indexed(palette),
            ..
        } = &image.inner.encoding
        else {
            panic!("not passed through");
        };
        assert_eq!(palette, &[10, 20, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        png.interlaced = true;
        let image = Image::from_bytes(png.encode()).unwrap();
        assert_eq!(decoded(&image).0, [10, 20, 30, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn ignores_data_after_the_image_and_transparency_where_it_is_not_allowed() {
        let png = Png::new(4, 4, 2, 8);
        let mut rows = png.rows();
        rows.extend_from_slice(&[0; 100]);
        assert!(Image::from_bytes(png.encode_with(&compress_to_vec_zlib(&rows, 6))).is_ok());
        let mut png = Png::new(4, 4, 6, 8);
        png.transparency = Some(vec![0, 0]);
        let image = Image::from_bytes(png.encode()).unwrap();
        let (_, expected_alpha) = png.expected();
        assert_eq!(decoded(&image).2, Some(&expected_alpha[..]));
    }

    /// The message of the error reading `data`.
    fn error(data: Vec<u8>) -> String {
        Image::from_bytes(data).unwrap_err().to_string()
    }

    #[test]
    fn refuses_damaged_png_files() {
        let png = Png::new(4, 3, 2, 8);
        let good = png.encode();
        let ihdr_crc = PNG_SIGNATURE.len() + 8 + 13;
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                good[..good.len() - 12].to_vec(),
                "the PNG file has no IEND chunk",
            ),
            (good[..45].to_vec(), "the PNG chunk at byte 33 is truncated"),
            (
                [&good[..33], b"\x80\0\0\0IDAT"].concat(),
                "the PNG chunk at byte 33 is too long",
            ),
            (
                {
                    let mut data = good.clone();
                    data[ihdr_crc] ^= 1;
                    data
                },
                "the PNG chunk IHDR at byte 8 has a wrong CRC",
            ),
            (
                {
                    let mut data = PNG_SIGNATURE.to_vec();
                    Png::chunk(&mut data, b"IDAT", b"");
                    data
                },
                "the PNG file does not start with an IHDR chunk",
            ),
            (
                {
                    let mut data = PNG_SIGNATURE.to_vec();
                    Png::chunk(&mut data, b"IHDR", &png.header()[..12]);
                    data
                },
                "the PNG IHDR chunk is not 13 bytes long",
            ),
            (
                {
                    let mut data = PNG_SIGNATURE.to_vec();
                    Png::chunk(&mut data, b"IHDR", &[&png.header()[..], &[0]].concat());
                    data
                },
                "the PNG IHDR chunk is not 13 bytes long",
            ),
            (
                png.encode_with(&compress_to_vec_zlib(&png.rows()[..30], 6)),
                "the PNG image data decompresses to 30 bytes, not 39",
            ),
            (
                png.encode_with(&compress_to_vec_zlib(&png.rows(), 6)[..8]),
                "the PNG image data is damaged",
            ),
            (
                {
                    let mut rows = png.rows();
                    rows[13] = 5;
                    png.encode_with(&compress_to_vec_zlib(&rows, 6))
                },
                "the PNG row at byte 13 of the image data has filter type 5",
            ),
        ];
        for (data, message) in cases {
            let err = error(data);
            assert!(
                err.starts_with(&format!("image data is damaged: {message}")),
                "{err}"
            );
        }
        let header = |width: u32, height: u32, bits: u8, color_type: u8, interlace: u8| {
            let mut png = Png::new(1, 1, 0, 8);
            png.width = width;
            png.height = height;
            png.bits = bits;
            png.color_type = color_type;
            let mut header = png.header();
            header[12] = interlace;
            let mut data = PNG_SIGNATURE.to_vec();
            Png::chunk(&mut data, b"IHDR", &header);
            Png::chunk(&mut data, b"IEND", b"");
            error(data)
        };
        assert_eq!(
            header(0, 1, 8, 0, 0),
            "image data is damaged: the PNG image is 0 x 1 pixels; each side must be 1 to 2^31 - 1"
        );
        assert!(header(1, 1 << 31, 8, 0, 0).contains("1 x 2147483648 pixels"));
        assert!(header(1, 1, 8, 5, 0).ends_with("the PNG colour type 5 does not exist"));
        assert!(
            header(1, 1, 4, 2, 0).ends_with("the PNG bit depth 4 is not allowed for colour type 2")
        );
        assert!(header(1, 1, 16, 3, 0).ends_with("bit depth 16 is not allowed for colour type 3"));
        assert!(header(1, 1, 8, 0, 2).ends_with("the PNG interlace method 2 does not exist"));

        let mut palette = Png::new(2, 2, 3, 8);
        palette.palette = None;
        assert!(error(palette.encode()).ends_with("the PNG palette image has no PLTE chunk"));
        palette.palette = Some(Vec::new());
        assert!(
            error(palette.encode())
                .ends_with("the PNG palette has 0 bytes, not 3 for each of 1 to 256 entries")
        );
        palette.palette = Some(vec![1, 2, 3, 4]);
        assert!(
            error(palette.encode())
                .ends_with("the PNG palette has 4 bytes, not 3 for each of 1 to 256 entries")
        );
        palette.palette = Some(vec![0; 771]);
        assert!(error(palette.encode()).contains("771 bytes"));
    }

    #[test]
    fn refuses_png_features_pdf_cannot_take() {
        let png = Png::new(2, 2, 0, 8);
        for (at, message) in [
            (10, "compression method 1 or filter method 0"),
            (11, "compression method 0 or filter method 1"),
        ] {
            let mut header = png.header();
            header[at] = 1;
            let mut data = PNG_SIGNATURE.to_vec();
            Png::chunk(&mut data, b"IHDR", &header);
            assert_eq!(
                error(data),
                format!("image data cannot be used: the PNG {message} is not the standard one, 0")
            );
        }
        let mut data = PNG_SIGNATURE.to_vec();
        Png::chunk(&mut data, b"IHDR", &png.header());
        Png::chunk(&mut data, b"ABCD", b"");
        assert_eq!(
            error(data),
            "image data cannot be used: the PNG file has a critical chunk ABCD this reader does \
             not know"
        );
        assert_eq!(
            error(b"GIF89a".to_vec()),
            "image data cannot be used: it is neither a JPEG nor a PNG file"
        );
    }

    #[test]
    fn refuses_png_files_whose_size_overflows() {
        // 2^31 - 1 rows of 6 × (2^31 - 1) bytes: more than 2^64 bytes of data. This
        // overflowed the arithmetic once.
        let mut png = Png::new(1, 1, 2, 16);
        png.width = 0x7FFF_FFFF;
        png.height = 0x7FFF_FFFF;
        let image = png.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(matches!(
            Image::from_bytes(image),
            Err(Error::ImageTooLarge { path: None })
        ));
    }

    #[test]
    fn counts_the_alpha_of_decoded_images_against_the_limit() {
        // 8256 × 8256 palette pixels: 68 MB of data, 204 MB decoded to RGB, 272.6 MB with
        // the alpha that partial transparency adds, over 2^28 bytes.
        let mut png = Png::new(1, 1, 3, 8);
        png.width = 8256;
        png.height = 8256;
        png.transparency = Some(vec![128]);
        let image = png.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(matches!(
            Image::from_bytes(image),
            Err(Error::ImageTooLarge { .. })
        ));
        // Without transparency it is passed through, and only its data counts.
        png.transparency = None;
        let image = png.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(error(image).contains("decompresses to"));
    }

    #[test]
    fn leaves_out_ancillary_chunks_with_a_wrong_crc_as_libpng_does() {
        let mut png = Png::new(3, 2, 3, 8);
        png.transparency = Some(vec![0]);
        let good = png.encode();
        let trns = good.windows(4).position(|w| w == b"tRNS").unwrap();
        // The CRC follows tRNS's one byte.
        let mut damaged = good.clone();
        damaged[trns + 5] ^= 1;
        let image = Image::from_bytes(damaged).unwrap();
        assert!(matches!(
            image.inner.encoding,
            Encoding::Png { mask: None, .. }
        ));
        // A critical chunk with a wrong CRC is still refused.
        let mut damaged = good;
        let plte = damaged.windows(4).position(|w| w == b"PLTE").unwrap();
        damaged[plte + 4] ^= 1;
        assert!(error(damaged).contains("the PNG chunk PLTE at byte"));
    }

    #[test]
    fn ignores_transparency_of_the_wrong_size() {
        // An empty tRNS for 16-bit grey: no key, so white stays opaque.
        let mut png = Png::new(2, 1, 0, 16);
        png.pixels = vec![vec![0xFFFF], vec![0]];
        png.transparency = Some(Vec::new());
        let image = Image::from_bytes(png.encode()).unwrap();
        assert!(matches!(
            image.inner.encoding,
            Encoding::Png { mask: None, .. }
        ));
        // A key of the wrong size for a colour image.
        let mut png = Png::new(2, 1, 2, 8);
        png.transparency = Some(vec![0, 0]);
        assert!(matches!(
            Image::from_bytes(png.encode()).unwrap().inner.encoding,
            Encoding::Png { mask: None, .. }
        ));
        // A palette tRNS longer than the palette is cut to its entries: index 2 does not
        // exist in a 1-bit image of two colours.
        let mut png = Png::new(2, 1, 3, 1);
        png.palette = Some(vec![0, 0, 0, 255, 255, 255]);
        png.transparency = Some(vec![255, 255, 0]);
        assert!(matches!(
            Image::from_bytes(png.encode()).unwrap().inner.encoding,
            Encoding::Png { mask: None, .. }
        ));
        // Nor does it when the palette has a third entry: a 1-bit index cannot reach it,
        // and a /Mask of [2 2] would be outside the samples' range.
        png.palette = Some(vec![0, 0, 0, 255, 255, 255, 9, 9, 9]);
        assert!(matches!(
            Image::from_bytes(png.encode()).unwrap().inner.encoding,
            Encoding::Png { mask: None, .. }
        ));
    }

    #[test]
    fn debug_output_leaves_out_the_file() {
        let image = Image::from_bytes(Png::new(64, 64, 2, 8).encode()).unwrap();
        let debug = format!("{image:?}");
        assert!(debug.starts_with("Image { bytes: "), "{debug}");
        assert!(debug.len() < 200, "{debug}");
    }

    #[test]
    fn refuses_png_files_too_large_to_decode_before_decompressing() {
        // 2^14 x 2^12 RGBA pixels at 8 bits, plus filter bytes: just over 2^28 bytes.
        let mut png = Png::new(1, 1, 6, 8);
        png.width = 1 << 14;
        png.height = 1 << 12;
        let image = png.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        let err = Image::from_bytes(image).unwrap_err();
        assert!(
            matches!(err, Error::ImageTooLarge { path: None }),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "image data is too large: its pixels would take more than 268435456 bytes"
        );
        // 2^29 pixels of 1 bit are 2^26 bytes of data, but decoded, with interlacing, they
        // would take 2^29 bytes.
        let mut small = Png::new(1, 1, 0, 1);
        small.width = 1 << 15;
        small.height = 1 << 14;
        small.interlaced = true;
        let image = small.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(matches!(
            Image::from_bytes(image),
            Err(Error::ImageTooLarge { .. })
        ));
        // Without interlacing, it is passed through: the size is fine.
        small.interlaced = false;
        let image = small.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(error(image).contains("decompresses to"));
        // Exactly at the limit is fine as far as the size goes: 2^28 - 1 grey pixels and a
        // filter byte.
        png.width = (1 << 28) - 1;
        png.height = 1;
        png.color_type = 0;
        let image = png.encode_with(&compress_to_vec_zlib(&[0; 10], 6));
        assert!(error(image).contains("decompresses to"));
    }

    /// A JPEG file up to its first scan: an optional APP14 segment, a frame header with
    /// marker `sof`, and padding before a marker. The scan codes the first component, all
    /// its coefficients or, in a progressive frame (`SOF2`), the DC coefficient.
    fn jpeg(
        sof: u8,
        precision: u8,
        width: u16,
        height: u16,
        components: u8,
        adobe: bool,
    ) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        // A JFIF segment, a restart marker and fill bytes, all skipped.
        data.extend_from_slice(b"\xFF\xE0\x00\x10JFIF\x00\x01\x02\x00\x00\x01\x00\x01\x00\x00");
        data.extend_from_slice(b"\xFF\xFF\xD0");
        if adobe {
            data.extend_from_slice(b"\xFF\xEE\x00\x0EAdobe\x00\x64\x00\x00\x00\x00\x02");
        }
        data.extend_from_slice(&[0xFF, sof]);
        let mut frame = vec![precision];
        frame.extend_from_slice(&height.to_be_bytes());
        frame.extend_from_slice(&width.to_be_bytes());
        frame.push(components);
        for id in 1..=components {
            frame.extend_from_slice(&[id, 0x11, 0]);
        }
        data.extend_from_slice(&((frame.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(&frame);
        // A DHT segment after the frame, and the scan.
        data.extend_from_slice(b"\xFF\xC4\x00\x03\x00\xFF\xDA\x00\x08\x01\x01\x00\x00");
        data.extend_from_slice(if sof == 0xC2 {
            b"\x00\x00"
        } else {
            b"\x3F\x00"
        });
        data.extend_from_slice(b"\x12\x34\xFF\x00\xFF\xD9");
        data
    }

    /// `data` with the byte at `at` replaced by `byte`.
    fn patched(data: &[u8], at: usize, byte: u8) -> Vec<u8> {
        let mut data = data.to_vec();
        data[at] = byte;
        data
    }

    /// The JPEG file `data` with a frame header `length` bytes long: its own, followed by
    /// `extra`.
    fn with_frame_length(data: &[u8], length: u16, extra: &[u8]) -> Vec<u8> {
        let frame = data.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        let end = frame + 2 + usize::from(u16_at(data, frame + 2).unwrap());
        let mut data = data.to_vec();
        data[frame + 2..frame + 4].copy_from_slice(&length.to_be_bytes());
        data.splice(end..end, extra.iter().copied());
        data
    }

    /// The JPEG file `data` with `scan` as its scan header, after the length, in place of
    /// its own.
    fn with_scan(data: &[u8], scan: &[u8]) -> Vec<u8> {
        let sos = data.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let end = sos + 2 + usize::from(u16_at(data, sos + 2).unwrap());
        let length = u16::try_from(scan.len() + 2).unwrap().to_be_bytes();
        [&data[..sos + 2], &length, scan, &data[end..]].concat()
    }

    #[test]
    fn accepts_jpeg_headers_at_their_limits() {
        // Four components, each with the largest sampling factors and table number,
        // scanned one at a time.
        let mut data = jpeg(0xC0, 8, 4, 4, 4, false);
        let frame = data.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        for component in 0..4 {
            data[frame + 11 + 3 * component] = 0x44;
            data[frame + 12 + 3 * component] = 3;
        }
        let image = Image::from_bytes(with_scan(&data, &[1, 1, 0, 0, 0x3F, 0])).unwrap();
        assert!(matches!(
            image.inner.encoding,
            Encoding::Jpeg { components: 4, .. }
        ));
        // All four in one scan, with the 10 blocks a minimum coded unit may have:
        // 2 × 2, 2 × 1, 1 × 2 and 1 × 2.
        let mut data = jpeg(0xC0, 8, 4, 4, 4, false);
        for (component, sampling) in [0x22, 0x21, 0x12, 0x12].into_iter().enumerate() {
            data[frame + 11 + 3 * component] = sampling;
        }
        let scan = [4, 1, 0, 2, 0x11, 3, 0x11, 4, 0x11, 0, 0x3F, 0];
        Image::from_bytes(with_scan(&data, &scan)).unwrap();
        // Identifiers the frame repeats, which T.81 does not allow but libjpeg decodes:
        // 1, 1, 2 scanned as 1, 1, 2 or one at a time, and 0, 0, 0 scanned together.
        let good = jpeg(0xC0, 8, 4, 4, 3, false);
        let repeated = patched(&patched(&good, frame + 13, 1), frame + 16, 2);
        let scans: [&[u8]; 3] = [
            &[3, 1, 0, 1, 0x11, 2, 0x11, 0, 0x3F, 0],
            &[1, 1, 0, 0, 0x3F, 0],
            &[1, 2, 0x11, 0, 0x3F, 0],
        ];
        for scan in scans {
            Image::from_bytes(with_scan(&repeated, scan)).unwrap();
        }
        // Scans of a frame 1, 2, 3 that libjpeg takes: 3, 2, out of the frame's order,
        // and 1, 3, which leaves a component out.
        for scan in [
            [2, 3, 0, 2, 0x11, 0, 0x3F, 0],
            [2, 1, 0, 3, 0x11, 0, 0x3F, 0],
        ] {
            Image::from_bytes(with_scan(&good, &scan)).unwrap();
        }
        // Blocks of the components the scan takes, not of those at its positions: 2, 3
        // of a frame 1 (4 × 4), 2, 3 interleaves 2 blocks.
        Image::from_bytes(with_scan(
            &patched(&good, frame + 11, 0x44),
            &[2, 2, 0, 3, 0x11, 0, 0x3F, 0],
        ))
        .unwrap();
        let zeros = patched(
            &patched(&patched(&good, frame + 10, 0), frame + 13, 0),
            frame + 16,
            0,
        );
        Image::from_bytes(with_scan(&zeros, &[3, 0, 0, 0, 0x11, 0, 0x11, 0, 0x3F, 0])).unwrap();
        // The smallest frame header: one component.
        let image = Image::from_bytes(jpeg(0xC1, 8, 1, 1, 1, false)).unwrap();
        assert_eq!((image.width(), image.height()), (1, 1));
    }

    #[test]
    fn reads_the_size_and_components_of_jpeg_files() {
        for sof in [0xC0, 0xC1, 0xC2] {
            for components in [1, 3, 4] {
                for adobe in [false, true] {
                    let image =
                        Image::from_bytes(jpeg(sof, 8, 640, 480, components, adobe)).unwrap();
                    assert_eq!((image.width(), image.height()), (640, 480));
                    assert!(matches!(
                        image.inner.encoding,
                        Encoding::Jpeg { components: c, adobe: a, end: None }
                            if c == components && a == adobe
                    ));
                    assert!(!image.needs_pdf_15());
                }
            }
        }
    }

    #[test]
    fn only_adobes_app14_segment_marks_inverted_cmyk() {
        let mut data = jpeg(0xC0, 8, 3, 2, 4, false);
        // An APP14 segment of another application, before the frame header.
        let frame = data.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        data.splice(
            frame..frame,
            *b"\xFF\xEE\x00\x0EOther\x00\x64\x00\x00\x00\x00\x02",
        );
        let image = Image::from_bytes(data).unwrap();
        assert!(matches!(
            image.inner.encoding,
            Encoding::Jpeg {
                components: 4,
                adobe: false,
                end: None
            }
        ));
        let object = String::from_utf8_lossy(&image.objects(1, Compression::Flate)[0]).into_owned();
        assert!(!object.contains("/Decode"), "{object}");
    }

    #[test]
    fn decoded_images_and_their_soft_masks_follow_the_compression() {
        // A decoded image that compresses: one colour, half transparent.
        let mut png = Png::new(16, 16, 6, 8);
        for pixel in &mut png.pixels {
            *pixel = vec![200, 10, 10, 128];
        }
        let image = Image::from_bytes(png.encode()).unwrap();
        let mut doc = crate::Document::new();
        doc.add_page(crate::PageSize::A4)
            .unwrap()
            .draw_image(
                &image,
                crate::Pt(0.0),
                crate::Pt(0.0),
                crate::Pt(10.0),
                crate::Pt(10.0),
            )
            .unwrap();
        let flate = "/Filter /FlateDecode /Type /XObject /Subtype /Image";
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert_eq!(pdf.matches(flate).count(), 2, "{pdf}");
        doc.set_compression(Compression::None);
        let pdf = String::from_utf8_lossy(&doc.to_bytes().unwrap()).into_owned();
        assert_eq!(pdf.matches(flate).count(), 0);
        assert_eq!(pdf.matches("/Subtype /Image").count(), 2);
    }

    #[test]
    fn writes_jpeg_files_as_they_are() {
        let objects = |components, adobe| {
            let data = jpeg(0xC0, 8, 3, 2, components, adobe);
            let image = Image::from_bytes(data.clone()).unwrap();
            let object = image.objects(7, Compression::Flate).concat();
            let head =
                String::from_utf8_lossy(&object[..object.len() - data.len() - 10]).into_owned();
            assert!(object.ends_with(&[&data[..], b"\nendstream"].concat()));
            head
        };
        assert_eq!(
            objects(3, false),
            format!(
                "<< /Length {} /Filter /DCTDecode /Type /XObject /Subtype /Image /Width 3 \
                 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 >>\nstream\n",
                jpeg(0xC0, 8, 3, 2, 3, false).len()
            )
        );
        assert!(objects(1, true).contains("/ColorSpace /DeviceGray /BitsPerComponent 8 >>"));
        assert!(objects(3, true).contains("/ColorSpace /DeviceRGB /BitsPerComponent 8 >>"));
        assert!(objects(4, false).contains("/ColorSpace /DeviceCMYK /BitsPerComponent 8 >>"));
        // Adobe's CMYK is inverted.
        assert!(
            objects(4, true)
                .contains("/DeviceCMYK /BitsPerComponent 8 /Decode [1 0 1 0 1 0 1 0] >>")
        );
    }

    #[test]
    fn refuses_jpeg_files_pdf_cannot_take_or_that_are_damaged() {
        let unsupported = [
            (
                jpeg(0xC3, 8, 4, 4, 1, false),
                "lossless, hierarchical and arithmetic-coded JPEG cannot go into a PDF",
            ),
            (jpeg(0xC5, 8, 4, 4, 1, false), "lossless, hierarchical"),
            (jpeg(0xC9, 8, 4, 4, 1, false), "arithmetic-coded"),
            (jpeg(0xCF, 8, 4, 4, 1, false), "arithmetic-coded"),
            (
                jpeg(0xC0, 12, 4, 4, 1, false),
                "12-bit JPEG cannot go into a PDF, only 8-bit",
            ),
            (
                jpeg(0xC0, 8, 4, 0, 1, false),
                "the JPEG file gives its height after its first scan",
            ),
            (
                jpeg(0xC0, 8, 4, 4, 2, false),
                "a JPEG file with 2 components cannot go into a PDF",
            ),
        ];
        for (data, message) in unsupported {
            let err = error(data);
            assert!(
                err.starts_with("image data cannot be used: ") && err.contains(message),
                "{err}"
            );
        }
        let good = jpeg(0xC0, 8, 4, 4, 3, false);
        let frame = good.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        let sos = good.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let damaged = [
            (
                jpeg(0xC0, 8, 0, 4, 1, false),
                "the JPEG frame header gives a width of 0",
            ),
            (
                b"\xFF\xD8\xFF\xDA".to_vec(),
                "the JPEG file has no frame header",
            ),
            (b"\xFF\xD8\xFF\xD9".to_vec(), "the JPEG file has no scan"),
            (
                b"\xFF\xD8\x00".to_vec(),
                "the JPEG file ends before its first scan",
            ),
            (
                b"\xFF\xD8\xFF\xFF".to_vec(),
                "the JPEG file ends before its first scan",
            ),
            (
                good[..frame + 3].to_vec(),
                &format!(
                    "the JPEG file ends in the length of its FFC0 segment at byte {}",
                    frame + 2
                ),
            ),
            (
                good[..frame + 8].to_vec(),
                &format!("the JPEG segment at byte {} is truncated", frame + 2),
            ),
            (
                [&good[..frame + 2], b"\x00\x05\x08\x00\x04"].concat(),
                "the JPEG frame header is truncated",
            ),
            // A frame header of 8 bytes that declares 3 components but describes none,
            // and one with a byte too many (B.2.2: Lf = 8 + 3 × Nf).
            (
                [
                    &good[..frame + 2],
                    b"\x00\x08\x08\x00\x04\x00\x04\x03",
                    &good[sos..],
                ]
                .concat(),
                "the JPEG frame header is 8 bytes long, not 17 for 3 components",
            ),
            (
                with_frame_length(&good, 18, b"\x00"),
                "the JPEG frame header is 18 bytes long, not 17 for 3 components",
            ),
            (
                patched(&good, frame + 14, 0x01),
                "the JPEG frame header gives component 2 the sampling factors 0 and 1, not 1 to 4",
            ),
            (
                patched(&good, frame + 14, 0x15),
                "the JPEG frame header gives component 2 the sampling factors 1 and 5, not 1 to 4",
            ),
            (
                patched(&good, frame + 14, 0x51),
                "the JPEG frame header gives component 2 the sampling factors 5 and 1, not 1 to 4",
            ),
            (
                patched(&good, frame + 14, 0x10),
                "the JPEG frame header gives component 2 the sampling factors 1 and 0, not 1 to 4",
            ),
            (
                patched(&good, frame + 15, 4),
                "the JPEG frame header gives component 2 quantization table 4, not 0 to 3",
            ),
            (
                jpeg(0xC0, 8, 4, 4, 0, false),
                "the JPEG frame header gives 0 components",
            ),
            // A scan marker with no header after it, or a header cut short by the end of
            // the file.
            (
                good[..sos + 2].to_vec(),
                &format!(
                    "the JPEG file ends in the length of its FFDA segment at byte {}",
                    sos + 2
                ),
            ),
            (
                good[..sos + 7].to_vec(),
                &format!("the JPEG segment at byte {} is truncated", sos + 2),
            ),
            // B.2.3: Ls = 6 + 2 × Ns, and a scan has 1 to 4 components.
            (
                patched(&good, sos + 4, 2),
                "the JPEG scan header is 8 bytes long, not 10 for 2 components",
            ),
            (
                patched(&good, sos + 4, 0),
                "the JPEG scan header gives 0 components, not 1 to 4",
            ),
            (
                with_scan(&good, &[5, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 0, 0x3F, 0]),
                "the JPEG scan header gives 5 components, not 1 to 4",
            ),
            (
                with_scan(&good, &[]),
                "the JPEG scan header gives 0 components, not 1 to 4",
            ),
            // A scan of components the frame does not have, or of one twice.
            (
                with_scan(&good, &[1, 9, 0, 0, 0x3F, 0]),
                "the JPEG scan header names component 9, which the frame header does not have",
            ),
            (
                with_scan(
                    &jpeg(0xC0, 8, 4, 4, 1, false),
                    &[2, 1, 0, 2, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan header names component 2, which the frame header does not have",
            ),
            (
                with_scan(&good, &[2, 1, 0, 1, 0x11, 0, 0x3F, 0]),
                "the JPEG scan header names component 1 more often than the frame header has it",
            ),
            // Components out of an order libjpeg takes: the scan's component k is the
            // frame's first one at position k or later with its identifier, so 1, 3, 2
            // and 3, 2, 1 fail at their last component.
            (
                with_scan(&good, &[3, 1, 0, 3, 0x11, 2, 0x11, 0, 0x3F, 0]),
                "the JPEG scan header names component 2 out of the frame header's order",
            ),
            (
                with_scan(&good, &[3, 3, 0, 2, 0x11, 1, 0x11, 0, 0x3F, 0]),
                "the JPEG scan header names component 1 out of the frame header's order",
            ),
            (
                with_scan(&good, &[2, 2, 0, 2, 0x11, 0, 0x3F, 0]),
                "the JPEG scan header names component 2 more often than the frame header has it",
            ),
            // With repeated identifiers, the component at position k or later is the one
            // taken: 2, 2 of a frame 1, 2, 2 takes the first 2 twice, and 1, 1 of a frame
            // 1, 1, 3 counts the blocks of the second 1, 4 × 4 (libjpeg: "Invalid
            // component ID 2 in SOS" and "Sampling factors too large for interleaved
            // scan").
            (
                with_scan(
                    &patched(&good, frame + 16, 2),
                    &[2, 2, 0, 2, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan header names component 2 out of the frame header's order",
            ),
            (
                with_scan(
                    &patched(&patched(&good, frame + 13, 1), frame + 14, 0x44),
                    &[2, 1, 0, 1, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan interleaves 17 blocks in each minimum coded unit, not at most 10",
            ),
            // A component taken before, not only the last one: 3, 2, 3 of a frame 1, 2, 3, 3
            // takes the first 3 twice (libjpeg: "Invalid component ID 3 in SOS").
            (
                with_scan(
                    &patched(&jpeg(0xC0, 8, 4, 4, 4, false), frame + 19, 3),
                    &[3, 3, 0, 2, 0x11, 3, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan header names component 3 out of the frame header's order",
            ),
            // A frame whose three components are all 1, scanned as 1, 2, 3.
            (
                with_scan(
                    &patched(&patched(&good, frame + 13, 1), frame + 16, 1),
                    &[3, 1, 0, 2, 0x11, 3, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan header names component 2, which the frame header does not have",
            ),
            // A second frame header (B.2.1), of the same kind or another.
            (
                [&good[..frame], &good[frame..frame + 19], &good[frame..]].concat(),
                &format!(
                    "the JPEG file has a second frame header, at byte {}",
                    frame + 19
                ),
            ),
            (
                [
                    &good[..frame + 19],
                    &patched(&good, frame + 1, 0xC2)[frame..],
                ]
                .concat(),
                &format!(
                    "the JPEG file has a second frame header, at byte {}",
                    frame + 19
                ),
            ),
            // Over 10 blocks in scans of two and three components: 4 × 3 + 1 and
            // 3 × 3 + 1 + 1.
            (
                with_scan(
                    &patched(&good, frame + 11, 0x43),
                    &[2, 1, 0, 2, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan interleaves 13 blocks in each minimum coded unit, not at most 10",
            ),
            (
                with_scan(
                    &patched(&good, frame + 11, 0x33),
                    &[3, 1, 0, 2, 0x11, 3, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan interleaves 11 blocks in each minimum coded unit, not at most 10",
            ),
            // 2 × 2 + 2 × 2 + 2 × 1 + 1 × 1 = 11 blocks in one scan.
            (
                with_scan(
                    &patched(
                        &patched(
                            &patched(&jpeg(0xC0, 8, 4, 4, 4, false), frame + 11, 0x22),
                            frame + 14,
                            0x22,
                        ),
                        frame + 17,
                        0x21,
                    ),
                    &[4, 1, 0, 2, 0x11, 3, 0x11, 4, 0x11, 0, 0x3F, 0],
                ),
                "the JPEG scan interleaves 11 blocks in each minimum coded unit, not at most 10",
            ),
            // Two bytes too many after a one-component scan header.
            (
                with_scan(&good, &[1, 1, 0, 0, 0x3F, 0, 0, 0]),
                "the JPEG scan header is 10 bytes long, not 8 for 1 component",
            ),
            (
                with_frame_length(&jpeg(0xC0, 8, 4, 4, 1, false), 12, b"\x01"),
                "the JPEG frame header is 12 bytes long, not 11 for 1 component",
            ),
        ];
        for (data, message) in damaged {
            assert_eq!(error(data), format!("image data is damaged: {message}"));
        }
    }

    #[test]
    fn checks_the_progressive_parameters_of_the_first_scan() {
        // Scans of the first of three components, or of all three, coding coefficients
        // Ss to Se with successive approximation Ah and Al.
        let scan = |all: bool, start: u8, end: u8, high: u8, low: u8| {
            let mut scan = if all {
                vec![3, 1, 0, 2, 0x11, 3, 0x11]
            } else {
                vec![1, 1, 0]
            };
            scan.extend_from_slice(&[start, end, high << 4 | low]);
            scan
        };
        let progressive = jpeg(0xC2, 8, 4, 4, 3, false);
        let accepted = [
            // DC coefficients interleaved, a first scan and refinements, Al at 13.
            scan(true, 0, 0, 0, 0),
            scan(true, 0, 0, 0, 13),
            scan(false, 0, 0, 1, 0),
            scan(true, 0, 0, 14, 13),
            // AC coefficients of one component: all, one, and the last.
            scan(false, 1, 63, 0, 0),
            scan(false, 5, 5, 0, 13),
            scan(false, 63, 63, 2, 1),
        ];
        for scan in accepted {
            Image::from_bytes(with_scan(&progressive, &scan)).unwrap_or_else(|err| {
                panic!("{scan:?}: {err}");
            });
        }
        let refused = [
            (
                scan(false, 0, 5, 0, 0),
                "the progressive JPEG scan header gives Ss 0 and Se 5, not 0 and 0 for the DC \
                 coefficients or 1 <= Ss <= Se <= 63 for AC coefficients",
            ),
            (scan(false, 0, 1, 0, 0), "gives Ss 0 and Se 1, not 0 and 0"),
            (scan(false, 2, 1, 0, 0), "gives Ss 2 and Se 1, not 0 and 0"),
            (
                scan(false, 1, 64, 0, 0),
                "gives Ss 1 and Se 64, not 0 and 0",
            ),
            (
                scan(false, 64, 64, 0, 0),
                "gives Ss 64 and Se 64, not 0 and 0",
            ),
            (
                scan(true, 1, 63, 0, 0),
                "the progressive JPEG scan header names 3 components in a scan of AC \
                 coefficients, not 1",
            ),
            // Two components, the first two of the frame.
            (
                vec![2, 1, 0, 2, 0x11, 1, 63, 0],
                "names 2 components in a scan of AC coefficients, not 1",
            ),
            (
                scan(false, 0, 0, 2, 0),
                "the progressive JPEG scan header gives Ah 2 and Al 0, not Al = Ah - 1 for a \
                 refinement",
            ),
            (
                scan(false, 1, 63, 1, 1),
                "gives Ah 1 and Al 1, not Al = Ah - 1",
            ),
            (
                scan(false, 0, 0, 13, 13),
                "gives Ah 13 and Al 13, not Al = Ah - 1",
            ),
            (
                scan(false, 0, 0, 0, 14),
                "the progressive JPEG scan header gives Al 14, not 0 to 13",
            ),
            (scan(true, 0, 0, 15, 14), "gives Al 14, not 0 to 13"),
            (scan(false, 1, 63, 0, 15), "gives Al 15, not 0 to 13"),
        ];
        for (scan, message) in &refused {
            let err = error(with_scan(&progressive, scan));
            assert!(
                err.starts_with("image data is damaged: ") && err.contains(*message),
                "{scan:?}: {err}"
            );
        }
        // libjpeg decodes the scan of a sequential frame whatever these fields hold.
        for sof in [0xC0, 0xC1] {
            let sequential = jpeg(sof, 8, 4, 4, 3, false);
            for (scan, _) in &refused {
                Image::from_bytes(with_scan(&sequential, scan)).unwrap_or_else(|err| {
                    panic!("{sof:#04X} {scan:?}: {err}");
                });
            }
        }
    }

    /// The JPEG file `data`, made by [`jpeg`], with `more` (segments and scans) before its
    /// end of image.
    fn with_more(data: &[u8], more: &[u8]) -> Vec<u8> {
        let end = data.len() - 2;
        assert_eq!(data[end..], [0xFF, 0xD9]);
        [&data[..end], more, &data[end..]].concat()
    }

    /// A scan with the header `scan`, after its length, and a little coded data: a stuffed
    /// zero, a restart marker and fill bytes, all part of it.
    fn scan_with(scan: &[u8]) -> Vec<u8> {
        let length = u16::try_from(scan.len() + 2).unwrap().to_be_bytes();
        [
            &[0xFF, 0xDA][..],
            &length,
            scan,
            b"\x56\xFF\x00\x78\xFF\xD3\x9A\xFF\xFF",
        ]
        .concat()
    }

    #[test]
    fn checks_the_scans_after_the_first() {
        // After the progressive DC scan of the first of three components, a scan of the AC
        // coefficients of one component, Ss to Se with Ah and Al.
        let progressive = jpeg(0xC2, 8, 4, 4, 3, false);
        let ac = |id: u8, start: u8, end: u8, high: u8, low: u8| {
            scan_with(&[1, id, 0x11, start, end, high << 4 | low])
        };
        let sound = ac(2, 1, 63, 0, 0);
        let bad = ac(2, 1, 64, 0, 0);
        let accepted = [
            sound.clone(),
            [ac(1, 1, 5, 0, 13), ac(1, 6, 63, 0, 0), ac(1, 1, 63, 14, 13)].concat(),
            // Segments and markers between scans: DHT, DQT, DRI, DNL, COM, APP1, TEM, a
            // restart marker and fill.
            [
                &b"\xFF\xC4\x00\x03\x10\xFF\xDB\x00\x03\x01\xFF\xDD\x00\x04\x00\x01"[..],
                &sound,
            ]
            .concat(),
            [
                &b"\xFF\xDC\x00\x04\x00\x04\xFF\xFE\x00\x03x\xFF\xE1\x00\x02"[..],
                &sound,
            ]
            .concat(),
            [&b"\xFF\x01\xFF\xD5\xFF\xFF"[..], &sound].concat(),
            // The body of a segment is skipped: a COM holding a start of image and a frame
            // header.
            [&b"\xFF\xFE\x00\x08\xFF\xD8\xFF\xC0\x00\x02"[..], &sound].concat(),
            // libjpeg reads nothing after the end of image.
            [&sound[..], b"\xFF\xD9", &bad].concat(),
        ];
        for more in accepted {
            Image::from_bytes(with_more(&progressive, &more)).unwrap_or_else(|err| {
                panic!("{more:02X?}: {err}");
            });
        }
        // A file that ends early ends the check: what follows is not read.
        let cut = [&progressive[..progressive.len() - 2], &sound].concat();
        Image::from_bytes(cut.clone()).unwrap();
        Image::from_bytes([&cut[..], &bad[..7]].concat()).unwrap();
        Image::from_bytes([&cut[..], &bad[..3]].concat()).unwrap();

        let refused = [
            (
                bad.clone(),
                "the progressive JPEG scan header gives Ss 1 and Se 64, not 0 and 0 for the DC \
                 coefficients or 1 <= Ss <= Se <= 63 for AC coefficients",
            ),
            (
                [&sound[..], &ac(3, 1, 63, 0, 14)].concat(),
                "the progressive JPEG scan header gives Al 14, not 0 to 13",
            ),
            (
                scan_with(&[3, 1, 0x11, 2, 0x11, 3, 0x11, 1, 63, 0]),
                "names 3 components in a scan of AC coefficients, not 1",
            ),
            (
                ac(4, 1, 63, 0, 0),
                "the JPEG scan header names component 4, which the frame header does not have",
            ),
            (
                scan_with(&[1, 2, 0x11, 1, 63]),
                "the JPEG scan header is 7 bytes long, not 8 for 1 component",
            ),
            // After DHT, COM, a restart marker and fill.
            (
                [
                    &b"\xFF\xC4\x00\x03\x10\xFF\xFE\x00\x03x\xFF\xD0\xFF\xFF"[..],
                    &bad,
                ]
                .concat(),
                "gives Ss 1 and Se 64",
            ),
            // After TEM, and after an APP1 segment holding an end of image.
            ([&b"\xFF\x01"[..], &bad].concat(), "gives Ss 1 and Se 64"),
            // After DQT, DRI and DNL.
            (
                [&b"\xFF\xDB\x00\x03\x01"[..], &bad].concat(),
                "gives Ss 1 and Se 64",
            ),
            (
                [&b"\xFF\xDD\x00\x04\x00\x00"[..], &bad].concat(),
                "gives Ss 1 and Se 64",
            ),
            (
                [&b"\xFF\xDC\x00\x04\x00\x10"[..], &bad].concat(),
                "gives Ss 1 and Se 64",
            ),
            (
                [&b"\xFF\xE1\x00\x04\xFF\xD9"[..], &bad].concat(),
                "gives Ss 1 and Se 64",
            ),
            (
                [
                    &b"\xFF\xC2\x00\x0B\x08\x00\x04\x00\x04\x01\x01\x11\x00"[..],
                    &sound,
                ]
                .concat(),
                "the JPEG file has a second frame header, at byte",
            ),
            (
                b"\xFF\xC0\x00\x0B\x08\x00\x04\x00\x04\x01\x01\x11\x00".to_vec(),
                "the JPEG file has a second frame header, at byte",
            ),
            (
                b"\xFF\xC3\x00\x0B\x08\x00\x04\x00\x04\x01\x01\x11\x00".to_vec(),
                "lossless, hierarchical and arithmetic-coded JPEG cannot go into a PDF",
            ),
            (
                b"\xFF\xD8".to_vec(),
                "the JPEG file has a second start of image, at byte",
            ),
            // A marker libjpeg does not know (#335), before a scan the check would refuse.
            (
                [&b"\xFF\x02\x00\x04ab"[..], &bad].concat(),
                "has marker FF02 at byte",
            ),
        ];
        for (more, message) in refused {
            let err = error(with_more(&progressive, &more));
            assert!(err.contains(message), "{more:02X?}: {err}");
        }

        // Sequential frames: each component in a scan of its own.
        let sequential = jpeg(0xC0, 8, 4, 4, 3, false);
        let scan = |ids: &[u8]| {
            let mut header = vec![u8::try_from(ids.len()).unwrap()];
            for &id in ids {
                header.extend_from_slice(&[id, 0x11]);
            }
            header.extend_from_slice(&[0, 63, 0]);
            scan_with(&header)
        };
        Image::from_bytes(with_more(&sequential, &[scan(&[2]), scan(&[3])].concat())).unwrap();
        Image::from_bytes(with_more(&sequential, &scan(&[2, 3]))).unwrap();
        assert!(
            error(with_more(
                &sequential,
                &[scan(&[2]), scan(&[2, 2])].concat()
            ))
            .contains("names component 2 more often than the frame header has it")
        );

        // A sequential frame whose first scan codes every component, grey or interleaved,
        // has no other scan: what libjpeg fails on after it is left out, and the file goes in
        // up to the end of the scan's coded data, with an end of image. A progressive frame
        // may have more scans.
        let written = |data: Vec<u8>| {
            let image = Image::from_bytes(data).unwrap();
            let object = image.objects(1, Compression::None).concat();
            let start = object.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
            object[start..object.len() - b"\nendstream".len()].to_vec()
        };
        let first = b"\xFF\xDA\x00\x08\x01\x01\x00\x00\x3F\x00";
        let at = sequential
            .windows(first.len())
            .position(|w| w == first)
            .unwrap();
        let interleaved = [
            &sequential[..at],
            b"\xFF\xDA\x00\x0C\x03\x01\x00\x02\x00\x03\x00\x00\x3F\x00",
            &sequential[at + first.len()..],
        ]
        .concat();
        for single in [
            jpeg(0xC0, 8, 4, 4, 1, false),
            jpeg(0xC1, 8, 4, 4, 1, false),
            interleaved,
        ] {
            // The helper's coded data ends right before its end of image.
            let end = single.len() - 2;
            for after in [
                scan(&[1]),
                [&b"\xFF\xFE\x00\x03x"[..], &scan(&[1])].concat(),
                scan_with(&[1, 1, 0x11, 0, 63]),
                b"\xFF\xC0\x00\x0B\x08\x00\x04\x00\x04\x01\x01\x11\x00".to_vec(),
                b"\xFF\xC3\x00\x0B\x08\x00\x04\x00\x04\x01\x01\x11\x00".to_vec(),
                b"\xFF\xD8".to_vec(),
                // A marker libjpeg does not know (#335).
                b"\xFF\x02\x00\x04ab".to_vec(),
            ] {
                assert_eq!(written(with_more(&single, &after)), single, "{after:02X?}");
            }
            // A restart marker after the coded data stays, and the fill before the next
            // marker goes; fill before a restart marker or a stuffed zero is coded data.
            assert_eq!(
                written(with_more(
                    &single,
                    &[&b"\xFF\xD3\xFF\xFF"[..], &scan(&[1])].concat()
                )),
                [&single[..end], b"\xFF\xD3\xFF\xD9"].concat()
            );
            let fill = b"\xFF\xFF\xD3\x56\xFF\xFF\x00\x78";
            assert_eq!(
                written(with_more(&single, &[&fill[..], &scan(&[1])].concat())),
                [&single[..end], fill, b"\xFF\xD9"].concat()
            );
            // What the check does not refuse stays as it is: a COM segment, and a segment cut
            // off by the end of the file (libjpeg fails on the last).
            for after in [&b"\xFF\xFE\x00\x03x"[..], b"\xFF\xDA\x00"] {
                let data = with_more(&single, after);
                assert_eq!(written(data.clone()), data, "{after:02X?}");
            }
        }
        // The coded data starts after the first scan header, which may hold marker bytes:
        // a sequential frame takes any Ss, Se, Ah and Al.
        let grey = jpeg(0xC0, 8, 4, 4, 1, false);
        let header = grey.windows(first.len()).position(|w| w == first).unwrap();
        let odd = [
            &grey[..header],
            b"\xFF\xDA\x00\x08\x01\x01\x00\x00\xFF\xD8",
            &grey[header + first.len()..],
        ]
        .concat();
        assert_eq!(written(odd.clone()), odd);
        // A sequential frame whose first scan leaves out a component is refused as before.
        assert!(
            error(with_more(
                &sequential,
                &[scan(&[2]), b"\xFF\xD8".to_vec()].concat()
            ))
            .contains("the JPEG file has a second start of image")
        );
        let progressive_grey = jpeg(0xC2, 8, 4, 4, 1, false);
        let data = with_more(&progressive_grey, &scan_with(&[1, 1, 0x11, 1, 63, 0]));
        assert_eq!(written(data.clone()), data);
        assert!(
            error(with_more(
                &progressive_grey,
                &scan_with(&[1, 1, 0x11, 1, 64, 0])
            ))
            .contains("gives Ss 1 and Se 64")
        );

        // A file main refused keeps its refusal: the later scans are checked last.
        let twelve_bit = jpeg(0xC2, 12, 4, 4, 3, false);
        assert_eq!(
            error(with_more(&twelve_bit, &bad)),
            error(twelve_bit.clone()),
        );
    }

    #[test]
    fn refuses_markers_libjpeg_does_not_know() {
        // T.81's reserved markers, JPG and JPGn: libjpeg stops at each, before the first
        // scan and after it (#335).
        let unknown = (0x02..=0xBF).chain([0xC8]).chain(0xF0..=0xFD);
        let baseline = jpeg(0xC0, 8, 4, 4, 1, false);
        let progressive = jpeg(0xC2, 8, 4, 4, 1, false);
        let ac = b"\xFF\xDA\x00\x08\x01\x01\x11\x01\x3F\x00\x12\x34";
        let mut count = 0;
        for code in unknown {
            let segment = [0xFF, code, 0x00, 0x04, b'a', b'b'];
            let message = format!("the JPEG file has marker FF{code:02X} at byte 2, which");
            let before = [&baseline[..2], &segment, &baseline[2..]].concat();
            let err = error(before);
            assert!(
                err.starts_with("image data is damaged: "),
                "{code:02X}: {err}"
            );
            assert!(err.contains(&message), "{code:02X}: {err}");
            // Between the scans of a progressive frame, before a scan it would take: the
            // segment goes in where the end of image was.
            let between = with_more(&progressive, &[&segment[..], ac].concat());
            let err = error(between);
            let message = format!(
                "image data is damaged: the JPEG file has marker FF{code:02X} at byte {}, which",
                progressive.len() - 2
            );
            assert!(err.starts_with(&message), "{code:02X}: {err}");
            // After the only scan of a sequential frame, which goes in without it.
            let after = with_more(&baseline, &segment);
            let image = Image::from_bytes(after).unwrap();
            assert!(
                matches!(image.inner.encoding, Encoding::Jpeg { end: Some(_), .. }),
                "{code:02X}"
            );
            count += 1;
        }
        assert_eq!(count, 0xBE + 1 + 14);
        // Every other marker is known: those with a segment are read or skipped by their
        // length before the frame header, and the rest are refused for what they are.
        for code in (0xC0..=0xFE).filter(|code| !matches!(code, 0xC8 | 0xF0..=0xFD)) {
            let segment = [0xFF, code, 0x00, 0x04, b'a', b'b'];
            let before = [&baseline[..2], &segment, &baseline[2..]].concat();
            if let Err(err) = Image::from_bytes(before) {
                assert!(
                    !err.to_string().contains("which T.81 reserves"),
                    "{code:02X}: {err}"
                );
            }
        }
    }

    /// A sequential 16 × 16 file whose scans code `scans` of its components, each component
    /// four blocks of nothing but zeros, with the restart interval `interval` (none for 0)
    /// given before its first scan, and `between` before each later one. Every scan's coded
    /// data is `3F FF D0 3F FF D1 3F FF D2 3F` with an interval of 1: the four blocks, each
    /// in two bits of 0, and the restart markers between them.
    fn restarted(components: u8, scans: &[&[u8]], interval: u16, between: &[u8]) -> Vec<u8> {
        let segment = |code: u8, body: &[u8]| {
            let length = u16::try_from(body.len() + 2).unwrap().to_be_bytes();
            [&[0xFF, code][..], &length, body].concat()
        };
        // A Huffman table with one code, 0, standing for 0: a DC difference of 0, or EOB.
        let one = [&[1][..], &[0; 15], &[0]].concat();
        let mut frame = vec![8, 0, 16, 0, 16, components];
        for id in 1..=components {
            frame.extend_from_slice(&[id, 0x11, 0]);
        }
        let mut data = [
            &b"\xFF\xD8"[..],
            &segment(0xDB, &[&[0][..], &[1; 64]].concat()),
            &segment(0xC0, &frame),
            &segment(0xC4, &[&[0x00][..], &one].concat()),
            &segment(0xC4, &[&[0x10][..], &one].concat()),
        ]
        .concat();
        if interval > 0 {
            data.extend(segment(0xDD, &interval.to_be_bytes()));
        }
        for (i, ids) in scans.iter().enumerate() {
            if i > 0 {
                data.extend_from_slice(between);
            }
            let mut header = vec![u8::try_from(ids.len()).unwrap()];
            for &id in *ids {
                header.extend_from_slice(&[id, 0]);
            }
            header.extend_from_slice(&[0, 0x3F, 0]);
            data.extend(segment(0xDA, &header));
            data.extend_from_slice(b"\x3F\xFF\xD0\x3F\xFF\xD1\x3F\xFF\xD2\x3F");
        }
        data.extend_from_slice(b"\xFF\xD9");
        data
    }

    /// `data` with `inserted` put before its `nth` (counting from 0) `3F FF D1`: before the
    /// second restart marker of a scan, `nth` scans on.
    fn before_restart(data: &[u8], nth: usize, inserted: &[u8]) -> Vec<u8> {
        let at = data
            .windows(3)
            .enumerate()
            .filter(|(_, w)| *w == b"\x3F\xFF\xD1")
            .nth(nth)
            .unwrap()
            .0
            + 1;
        [&data[..at], inserted, &data[at..]].concat()
    }

    /// `data` with `inserted` put before the `rst` (counting from 0, at most 2) restart
    /// marker of the `nth` scan (counting from 0), whose coded data is `restarted`'s.
    fn before_restart_n(data: &[u8], nth: usize, rst: u8, inserted: &[u8]) -> Vec<u8> {
        let marker = [0x3F, 0xFF, 0xD0 + rst];
        let at = data
            .windows(3)
            .enumerate()
            .filter(|(_, w)| *w == marker)
            .nth(nth)
            .unwrap()
            .0
            + 1;
        [&data[..at], inserted, &data[at..]].concat()
    }

    #[test]
    fn counts_the_restart_markers_of_a_scan_by_its_mcus() {
        // 4:2:0: the first component sampled 2 × 2, the others 1 × 1 (A.2).
        let frame = |width: u16| Frame {
            precision: 8,
            height: 32,
            width,
            components: vec![(1, 2, 2), (2, 1, 1), (3, 1, 1)],
            progressive: false,
        };
        let one = |id: u8| [1, id, 0, 0, 63, 0];
        let all = [3, 1, 0, 2, 0, 3, 0, 0, 63, 0];
        let square = frame(32);
        // A scan of one component has a block an MCU: 4 × 4 of the first, 2 × 2 of the
        // others; a scan of all of them an MCU of 16 × 16 pixels, 2 × 2 of them.
        assert_eq!(scan_restarts(&square, &one(1), 1), 15);
        assert_eq!(scan_restarts(&square, &one(1), 4), 3);
        assert_eq!(scan_restarts(&square, &one(1), 5), 3);
        assert_eq!(scan_restarts(&square, &one(2), 1), 3);
        assert_eq!(scan_restarts(&square, &all, 1), 3);
        assert_eq!(scan_restarts(&square, &all, 4), 0);
        assert_eq!(scan_restarts(&square, &all, 0), 0);
        // 33 pixels wide: 5 columns of blocks of the first component, 3 of the others (17
        // samples), and 3 columns of MCUs of a scan of all.
        let wide = frame(33);
        assert_eq!(scan_restarts(&wide, &one(1), 1), 5 * 4 - 1);
        assert_eq!(scan_restarts(&wide, &one(3), 1), 3 * 2 - 1);
        assert_eq!(scan_restarts(&wide, &all, 1), 3 * 2 - 1);
        // A component the frame does not have.
        assert_eq!(scan_restarts(&wide, &one(4), 1), 0);
        // A scan of two of the three components is a scan of several.
        assert_eq!(scan_restarts(&square, &[2, 1, 0, 2, 0, 0, 63, 0], 1), 3);
        // 40 pixels high: three rows of MCUs of 16 pixels.
        let tall = Frame {
            height: 40,
            ..frame(32)
        };
        assert_eq!(scan_restarts(&tall, &all, 1), 2 * 3 - 1);
        // 4:2:2, 48 × 40: the first component sampled 2 × 1, MCUs of 16 × 8 pixels, and
        // blocks of 24 × 40 samples for the others.
        let wide_mcus = Frame {
            height: 40,
            width: 48,
            components: vec![(1, 2, 1), (2, 1, 1), (3, 1, 1)],
            progressive: false,
            precision: 8,
        };
        assert_eq!(scan_restarts(&wide_mcus, &all, 1), 3 * 5 - 1);
        assert_eq!(scan_restarts(&wide_mcus, &one(1), 1), 6 * 5 - 1);
        assert_eq!(scan_restarts(&wide_mcus, &one(2), 1), 3 * 5 - 1);
    }

    #[test]
    fn skips_unknown_markers_before_a_restart_marker_as_libjpeg_does() {
        // In a scan with a restart interval, libjpeg skips a marker of 0x02 to 0xBF where
        // a restart marker follows it, and draws the whole scan; `qpdf --check` passes such
        // files (#411). Without the interval it fails on the marker.
        let reserved = b"\xFF\x02\x00\x04ab";
        let three = restarted(3, &[&[1], &[2], &[3]], 1, b"");
        let whole = |data: Vec<u8>| {
            let image = Image::from_bytes(data.clone()).unwrap_or_else(|err| panic!("{err}"));
            matches!(image.inner.encoding, Encoding::Jpeg { end: None, .. })
        };
        assert!(whole(three.clone()));
        for code in 0x02..=0xBF {
            let marker = [0xFF, code, 0x00, 0x04, b'a', b'b'];
            for scan in 0..3 {
                assert!(
                    whole(before_restart(&three, scan, &marker)),
                    "{code:02X} {scan}"
                );
            }
        }
        // Several such markers, fill before them and after them, and stuffed zeros between
        // them.
        for inserted in [
            &b"\xFF\x02\x00\x04ab\xFF\x03\x12"[..],
            b"\xFF\xFF\xFF\x02\x00\x04ab",
            b"\xFF\x02\x00\x04ab\xFF\xFF",
            b"\xFF\x02\xFF\x00\x34\xFF\x01",
        ] {
            assert!(
                whole(before_restart(&three, 1, inserted)),
                "{inserted:02X?}"
            );
        }
        // libjpeg looks for the next marker past stuffed zeros and fill: here a second
        // unknown marker after a stuffed zero, and, after fill, a marker before the next
        // restart marker, both skipped.
        let first_rst2 = three.windows(3).position(|w| w == b"\x3F\xFF\xD2").unwrap() + 1;
        let first_rst1 = three.windows(3).position(|w| w == b"\x3F\xFF\xD1").unwrap() + 1;
        for data in [
            [
                &three[..first_rst1],
                b"\xFF\x02\x00\x04ab\xFF\x00\xFF\x03\x00\x04ab",
                &three[first_rst1..],
            ]
            .concat(),
            [
                &three[..first_rst1],
                b"\xFF\x02\x00\x04ab\xFF\xFF",
                &three[first_rst1..first_rst2],
                b"\xFF\x03\x00\x04ab",
                &three[first_rst2..],
            ]
            .concat(),
        ] {
            assert!(whole(data));
        }
        // When the next marker is not a restart marker, the scan ends there, and libjpeg
        // reads on from that marker: the next scan, the end of image, a segment, or a
        // restart marker damaged into one it does not know, such as RST2 with bit 7 off.
        let second = three.windows(2).rposition(|w| w == b"\xFF\xDA").unwrap();
        let last_rst1 = three
            .windows(3)
            .rposition(|w| w == b"\x3F\xFF\xD1")
            .unwrap()
            + 1;
        let first_rst2 = three.windows(3).position(|w| w == b"\x3F\xFF\xD2").unwrap() + 2;
        let mut flipped = three.clone();
        flipped[first_rst2] = 0x52;
        for data in [
            flipped,
            [&three[..first_rst1], reserved, &three[second..]].concat(),
            [&three[..last_rst1], reserved, b"\xFF\xD9"].concat(),
            before_restart(&three, 0, b"\xFF\x02\x00\x04ab\xFF\xFE\x00\x03x"),
        ] {
            assert!(whole(data));
        }
        // Refused where libjpeg fails: without a restart interval, with one of 0, given
        // between scans or after another before the first, and after the last restart
        // marker of a scan.
        let no_interval = restarted(3, &[&[1], &[2], &[3]], 0, b"");
        let offset = |data: &[u8]| data.windows(6).position(|w| w == reserved).unwrap();
        let sos = no_interval
            .windows(2)
            .position(|w| w == b"\xFF\xDA")
            .unwrap();
        let zero_after_one = [
            &no_interval[..sos],
            b"\xFF\xDD\x00\x04\x00\x01\xFF\xDD\x00\x04\x00\x00",
            &no_interval[sos..],
        ]
        .concat();
        for data in [
            before_restart(&no_interval, 0, reserved),
            {
                let off = restarted(3, &[&[1], &[2], &[3]], 1, b"\xFF\xDD\x00\x04\x00\x00");
                before_restart(&off, 1, reserved)
            },
            before_restart(&zero_after_one, 0, reserved),
            {
                // The first scan's header takes 10 bytes, and its last interval starts 9
                // bytes into its coded data, after RST2.
                let at = three.windows(2).position(|w| w == b"\xFF\xDA").unwrap() + 19;
                assert_eq!(three[at - 2..at + 1], *b"\xFF\xD2\x3F");
                [&three[..at], reserved, &three[at..]].concat()
            },
        ] {
            let at = offset(&data);
            let err = error(data);
            assert!(
                err.starts_with(&format!(
                    "image data is damaged: the JPEG file has marker FF02 at byte {at}, which"
                )),
                "{err}"
            );
        }
        // A marker that ends the scan is read as it is anywhere: JPG is refused.
        let jpg = before_restart(&three, 0, b"\xFF\x02\x00\x04ab\xFF\xC8\x00\x04ab");
        let at = jpg.windows(2).position(|w| w == b"\xFF\xC8").unwrap();
        assert!(error(jpg).contains(&format!("marker FFC8 at byte {at}")));
        // libjpeg skips it only before a restart marker the scan has still to come: after
        // a scan's last interval, a restart marker more than its MCUs need does not save it.
        let extra = restarted(3, &[&[1], &[2], &[3]], 1, b"\xFF\x02\x00\x04ab\xFF\xD3");
        let at = offset(&extra);
        assert!(error(extra).contains(&format!("marker FF02 at byte {at}")));
        // The restart marker libjpeg finds after a skipped marker is one of those it
        // expects: skipped before the scan's last one, a second marker after it is not.
        let last = three.windows(3).position(|w| w == b"\x3F\xFF\xD2").unwrap() + 1;
        let end = last + 3;
        let twice = [
            &three[..last],
            reserved,
            &three[last..end],
            b"\xFF\x03\x00\x04ab\xFF\xD3",
            &three[end..],
        ]
        .concat();
        let at = twice.windows(2).position(|w| w == b"\xFF\x03").unwrap();
        assert!(error(twice).contains(&format!("marker FF03 at byte {at}")));
        // Nor does one after the segment that ends a scan whose coded data stops early: a
        // 16 × 32 frame, whose scans have eight MCUs, with three of their seven restart
        // markers.
        let mut headers = restarted(
            3,
            &[&[1], &[2], &[3]],
            1,
            b"\xFF\xFE\x00\x03x\xFF\x02\x00\x04ab\xFF\xD3",
        );
        let sof = headers.windows(2).position(|w| w == b"\xFF\xC0").unwrap();
        // The frame's height, after its marker, length and precision.
        headers[sof + 5..sof + 7].copy_from_slice(&32u16.to_be_bytes());
        let at = offset(&headers);
        assert!(error(headers).contains(&format!("marker FF02 at byte {at}")));
        // An interval given between scans sets it for the scans after it.
        let later = restarted(3, &[&[1], &[2], &[3]], 0, b"\xFF\xDD\x00\x04\x00\x01");
        assert!(whole(before_restart(&later, 1, reserved)));
        assert!(error(before_restart(&later, 0, reserved)).contains("marker FF02"));

        // Restart markers go by their numbers, 0 to 7 in turn. RST0 twice: the second is
        // one back, skipped, so RST1 and RST2 still come, and a marker before RST2 is
        // skipped.
        let doubled = [
            &three[..first_rst1 - 3],
            b"\xFF\xD0",
            &three[first_rst1 - 3..],
        ]
        .concat();
        assert_eq!(
            doubled[first_rst1 - 3..first_rst1 + 1],
            *b"\xFF\xD0\xFF\xD0"
        );
        assert!(whole(before_restart_n(&doubled, 0, 2, reserved)));
        // RST1 missing: RST2 is one further on, so the restart point passes and RST2 is
        // taken at the next; none is left for a marker after it, with another restart
        // marker after that.
        let missing = [&three[..first_rst1], &three[first_rst1 + 2..]].concat();
        let after_last = missing
            .windows(3)
            .position(|w| w == b"\xFF\xD2\x3F")
            .unwrap()
            + 3;
        let late = [
            &missing[..after_last],
            b"\xFF\x02\x00\x04ab\xFF\xD3",
            &missing[after_last..],
        ]
        .concat();
        let at = offset(&late);
        assert!(error(late).contains(&format!("marker FF02 at byte {at}")));

        // A file whose only scan codes every component goes in whole when the marker is
        // skipped, and is cut before it otherwise. Cut for what follows its scan, it keeps
        // the marker libjpeg skips in its coded data.
        let grey = restarted(1, &[&[1]], 1, b"");
        let marked = before_restart(&grey, 0, reserved);
        assert!(whole(marked.clone()));
        let end = |data: Vec<u8>| match Image::from_bytes(data).unwrap().inner.encoding {
            Encoding::Jpeg { end, .. } => end,
            _ => unreachable!(),
        };
        let unmarked = before_restart(&restarted(1, &[&[1]], 0, b""), 0, reserved);
        assert_eq!(end(unmarked.clone()), Some(offset(&unmarked)));
        let followed = with_more(&marked, b"\xFF\xD8");
        assert_eq!(end(followed), Some(marked.len() - 2));
        // The cut counts the restart markers as the check does: it ends at a second marker
        // after the last restart marker, at a marker libjpeg fails on after a skipped
        // one, and at a marker after the scan's last interval.
        let grey_last = grey.windows(3).position(|w| w == b"\x3F\xFF\xD2").unwrap() + 1;
        let grey_end = grey_last + 3;
        let twice = [
            &grey[..grey_last],
            reserved,
            &grey[grey_last..grey_end],
            b"\xFF\x03\x00\x04ab\xFF\xD3",
            &grey[grey_end..],
        ]
        .concat();
        let at = twice.windows(2).position(|w| w == b"\xFF\x03").unwrap();
        assert_eq!(end(twice), Some(at));
        let jpg = before_restart(&grey, 0, b"\xFF\x02\x00\x04ab\xFF\xC8\x00\x04ab");
        let at = jpg.windows(2).position(|w| w == b"\xFF\xC8").unwrap();
        assert_eq!(end(jpg), Some(at));
        let extra = with_more(&grey, b"\xFF\x02\x00\x04ab\xFF\xD3");
        assert_eq!(end(extra.clone()), Some(offset(&extra)));
        // An interval of more than 255 MCUs: a grey 160 × 160 frame has 400, so an interval
        // of 257 has one restart marker, after which a marker is not skipped.
        let mut big = restarted(1, &[&[1]], 257, b"");
        let sof = big.windows(2).position(|w| w == b"\xFF\xC0").unwrap();
        big[sof + 5..sof + 9].copy_from_slice(b"\x00\xA0\x00\xA0");
        let rst1 = big.windows(3).position(|w| w == b"\x3F\xFF\xD1").unwrap() + 1;
        let big = [&big[..rst1], reserved, &big[rst1..]].concat();
        assert_eq!(end(big.clone()), Some(offset(&big)));
    }

    #[test]
    fn refuses_hierarchical_jpeg_files() {
        // A hierarchical file starts with a DHP segment (B.3), here before a baseline frame.
        for marker in [0xDE, 0xDF] {
            let mut data = vec![0xFF, 0xD8, 0xFF, marker, 0x00, 0x02];
            data.extend_from_slice(&jpeg(0xC0, 8, 4, 4, 1, false)[2..]);
            assert!(
                error(data).ends_with(
                    "lossless, hierarchical and arithmetic-coded JPEG cannot go into a PDF"
                ),
                "{marker:X}"
            );
        }
    }

    #[test]
    fn skips_bytes_before_a_jpeg_marker_as_libjpeg_does() {
        let good = jpeg(0xC0, 8, 4, 4, 3, false);
        let frame = good.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        for stray in [&b"\x00\x12garbage"[..], b"\x12\xFF\x00\x56\x78"] {
            let data = [&good[..frame], stray, &good[frame..]].concat();
            let image = Image::from_bytes(data).unwrap();
            assert_eq!((image.width(), image.height()), (4, 4));
        }
    }

    #[test]
    fn writes_a_range_of_transparent_palette_entries_as_a_colour_key() {
        let mut png = Png::new(3, 2, 3, 2);
        png.transparency = Some(vec![255, 0, 0]);
        let image = Image::from_bytes(png.encode()).unwrap();
        let object = String::from_utf8_lossy(&image.objects(1, Compression::None)[0]).into_owned();
        assert!(
            object.contains("/BitsPerComponent 2 /Mask [1 2] >>"),
            "{object}"
        );
    }

    #[test]
    fn writes_the_data_of_one_png_idat_chunk_or_several_as_it_is() {
        // Chunks large enough to be read from the file, not copied.
        let png = Png::new(40, 30, 2, 8);
        let data = compress_to_vec_zlib(&png.rows(), 6);
        let (first, second) = data.split_at(1000);
        // One chunk; empty chunks around and between two; three.
        let layouts: [&[&[u8]]; 3] = [
            &[&data],
            &[b"", first, b"", second, b""],
            &[first, &second[..1], &second[1..]],
        ];
        for chunks in layouts {
            let mut file = PNG_SIGNATURE.to_vec();
            Png::chunk(&mut file, b"IHDR", &png.header());
            for chunk in chunks {
                Png::chunk(&mut file, b"IDAT", chunk);
            }
            Png::chunk(&mut file, b"IEND", b"");
            let image = Image::from_bytes(file).unwrap();
            assert!(
                matches!(
                    &image.inner.encoding,
                    Encoding::Png {
                        data: PngData::InFile(_),
                        ..
                    }
                ),
                "{chunks:?}"
            );
            let object = image.objects(1, Compression::Flate).concat();
            // The stream's data is the zlib stream, however the file cut it.
            let head = format!(
                "<< /Length {} /Filter /FlateDecode /DecodeParms",
                data.len()
            );
            assert!(object.starts_with(head.as_bytes()), "{chunks:?}");
            assert!(
                object.ends_with(&[&data[..], b"\nendstream"].concat()),
                "{chunks:?}"
            );
        }
    }

    /// A PNG file of `png`'s header, with `data` cut into `IDAT` chunks of `size` bytes, the
    /// last one taking what is left over too.
    fn png_in_chunks(png: &Png, data: &[u8], size: usize) -> Vec<u8> {
        let mut file = PNG_SIGNATURE.to_vec();
        Png::chunk(&mut file, b"IHDR", &png.header());
        let whole = (data.len() / size).max(1);
        for i in 0..whole {
            let end = if i + 1 == whole {
                data.len()
            } else {
                (i + 1) * size
            };
            Png::chunk(&mut file, b"IDAT", &data[i * size..end]);
        }
        Png::chunk(&mut file, b"IEND", b"");
        file
    }

    #[test]
    fn keeps_a_copy_of_png_data_in_chunks_too_small_to_point_at() {
        // Chunks of 1 byte would take 16 bytes each to point at: the data is copied once.
        // Chunks of 100 bytes are pointed at in the file.
        let png = Png::new(40, 30, 2, 8);
        let data = compress_to_vec_zlib(&png.rows(), 6);
        // At the limit, chunks of 64 bytes, whose places take a quarter of their data, are
        // pointed at, and chunks of 63 bytes are copied (with what is left over added to
        // the last chunk, so that every chunk but the last has that size).
        for (size, copied) in [(1, true), (100, false), (64, false), (63, true)] {
            let image = Image::from_bytes(png_in_chunks(&png, &data, size)).unwrap();
            let Encoding::Png { data: kept, .. } = &image.inner.encoding else {
                panic!("not passed through");
            };
            assert_eq!(matches!(kept, PngData::Copied(_)), copied, "{size}");
            assert_eq!(*image.inner.png_data(kept), data, "{size}");
            let object = image.objects(1, Compression::Flate).concat();
            assert!(
                object.ends_with(&[&data[..], b"\nendstream"].concat()),
                "{size}"
            );
        }
    }

    #[test]
    fn writes_png_data_as_it_is_whatever_the_compression() {
        // Stored deflate blocks, which Flate would shrink a lot: the PNG's own zlib stream
        // goes into the PDF unchanged, with one filter, whatever the document's setting.
        let mut png = Png::new(64, 64, 0, 8);
        for pixel in &mut png.pixels {
            *pixel = vec![7];
        }
        let data = compress_to_vec_zlib(&png.rows(), 0);
        assert!(compress_to_vec_zlib(&data, 6).len() < data.len() / 2);
        let image = Image::from_bytes(png_in_chunks(&png, &data, data.len())).unwrap();
        for compression in [Compression::Flate, Compression::None] {
            let object = image.objects(1, compression).concat();
            let text = String::from_utf8_lossy(&object);
            assert_eq!(text.matches("/Filter").count(), 1, "{compression:?}");
            assert!(
                object.ends_with(&[&data[..], b"\nendstream"].concat()),
                "{compression:?}"
            );
        }
    }

    #[test]
    fn writes_png_files_with_their_predictors_or_decoded_with_a_soft_mask() {
        let mut png = Png::new(3, 2, 3, 4);
        png.palette = Some(vec![1, 2, 3, 4, 5, 6]);
        png.transparency = Some(vec![0]);
        let image = Image::from_bytes(png.encode()).unwrap();
        let objects = image.objects(9, Compression::Flate);
        assert_eq!(objects.len(), 1);
        let text = String::from_utf8_lossy(&objects[0]).into_owned();
        let palette = format!("010203040506{}", "000000".repeat(14));
        assert!(
            text.starts_with(&format!(
                "<< /Length {} /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 1 \
                 /BitsPerComponent 4 /Columns 3 >> /Type /XObject /Subtype /Image /Width 3 \
                 /Height 2 /ColorSpace [/Indexed /DeviceRGB 15 <{palette}>] /BitsPerComponent 4 \
                 /Mask [0 0] >>\nstream\n",
                compress_to_vec_zlib(&png.rows(), 6).len()
            )),
            "{text}"
        );

        let png = Png::new(3, 2, 6, 8);
        let image = Image::from_bytes(png.encode()).unwrap();
        let objects = image.objects(9, Compression::None);
        let (samples, alpha) = png.expected();
        assert_eq!(
            objects,
            [
                [
                    b"<< /Length 18 /Type /XObject /Subtype /Image /Width 3 /Height 2 \
                      /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 10 0 R >>\nstream\n"
                        .as_slice(),
                    &samples,
                    b"\nendstream",
                ]
                .concat(),
                [
                    b"<< /Length 6 /Type /XObject /Subtype /Image /Width 3 /Height 2 \
                      /ColorSpace /DeviceGray /BitsPerComponent 8 >>\nstream\n"
                        .as_slice(),
                    &alpha,
                    b"\nendstream",
                ]
                .concat(),
            ]
        );
    }

    #[test]
    fn images_are_equal_when_their_data_is() {
        let data = Png::new(2, 2, 0, 8).encode();
        let image = Image::from_bytes(data.clone()).unwrap();
        assert_eq!(image, image.clone());
        assert_eq!(image, Image::from_bytes(data).unwrap());
        assert_ne!(
            image,
            Image::from_bytes(Png::new(2, 3, 0, 8).encode()).unwrap()
        );
    }

    #[test]
    fn computes_the_png_crc() {
        // The CRC of IEND, which every PNG file ends with.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn names_the_file_of_an_image_error() {
        let dir = std::env::temp_dir().join(format!("officewright-image-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.png");
        fs::write(&path, PNG_SIGNATURE).unwrap();
        let err = Image::from_file(&path).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "image {} is damaged: the PNG file has no IEND chunk",
                path.display()
            )
        );
        let missing = dir.join("missing.png");
        assert!(matches!(
            Image::from_file(&missing),
            Err(Error::Read { .. })
        ));
        fs::remove_dir_all(&dir).unwrap();
    }
}
