//! Raster images: JPEG files passed through with the DCTDecode filter, and PNG files passed
//! through with the PNG predictors of the FlateDecode filter where PDF can show them as
//! they are, decoded otherwise (ISO 32000-1, 8.9 and 7.4.4.4).

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
/// A JPEG file goes into the PDF as it is. So does a PNG file whenever PDF can show it
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
    /// The JPEG file as it is.
    Jpeg {
        components: u8,
        /// The file has Adobe's `APP14` marker, so its CMYK samples are inverted.
        adobe: bool,
    },
    /// The zlib stream of a PNG file's `IDAT` chunks, whose rows the PNG predictors undo.
    Png {
        data: Vec<u8>,
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
            Self::Jpeg { components, adobe } => f
                .debug_struct("Jpeg")
                .field("components", components)
                .field("adobe", adobe)
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
            Encoding::Jpeg { components, adobe } => {
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
                vec![stream_object(&entries, &inner.source, Compression::None)]
            }
            Encoding::Png {
                data,
                space,
                bits,
                mask,
            } => {
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
                vec![stream_object(&entries, data, Compression::None)]
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
/// and whether it has Adobe's `APP14` segment. The frame header and the first scan header
/// are checked as libjpeg checks them when it reads them, so that a file a viewer cannot
/// start to decode is refused; damage in the coded data is not looked for.
fn read_jpeg(data: &[u8]) -> std::result::Result<(u32, u32, Encoding), Problem> {
    let mut at = 2;
    let mut frame = None;
    let mut adobe = false;
    let frame = loop {
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
            _ => {}
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
            break frame;
        }
        match code {
            // Baseline, extended sequential and progressive Huffman coding.
            0xC0..=0xC2 => frame = Some(read_frame(segment)?),
            // DHT, JPG and DAC are not frame headers.
            0xC4 | 0xC8 | 0xCC => {}
            // Lossless, differential and arithmetic-coded frames, and the DHP and EXP
            // markers of hierarchical files (B.3, Annex J).
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDE | 0xDF => {
                return Err(unsupported(
                    "lossless, hierarchical and arithmetic-coded JPEG cannot go into a PDF",
                ));
            }
            0xEE => adobe |= segment.starts_with(b"Adobe"),
            _ => {}
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
    Ok((
        u32::from(frame.width),
        u32::from(frame.height),
        Encoding::Jpeg { components, adobe },
    ))
}

/// A JPEG frame header (ITU T.81, B.2.2).
struct Frame {
    precision: u8,
    height: u16,
    width: u16,
    /// Each component's identifier and horizontal and vertical sampling factors.
    components: Vec<(u8, u8, u8)>,
}

/// Reads a JPEG frame header, `segment` after its length, and checks it as libjpeg does:
/// 1 to 255 components of three bytes each, so that the header is `8 + 3 × components`
/// bytes long, with sampling factors of 1 to 4 and a quantization table of 0 to 3.
fn read_frame(segment: &[u8]) -> std::result::Result<Frame, Problem> {
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
    })
}

/// Checks a JPEG scan header of `length` bytes, `segment` after the length, against its
/// `frame` as libjpeg does (ITU T.81, B.2.3): 1 to 4 components, so that the header is
/// `6 + 2 × components` bytes long, each a component of the frame named once, and in an
/// interleaved scan at most 10 blocks in each minimum coded unit.
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
    let mut named = Vec::with_capacity(usize::from(count));
    let mut blocks = 0;
    for specification in segment.get(1..).unwrap_or_default().chunks_exact(2) {
        let Some(&id) = specification.first() else {
            continue;
        };
        if named.len() == usize::from(count) {
            break;
        }
        if named.contains(&id) {
            return Err(malformed(format!(
                "the JPEG scan header names component {id} twice"
            )));
        }
        let Some(&(_, horizontal, vertical)) =
            frame.components.iter().find(|component| component.0 == id)
        else {
            return Err(malformed(format!(
                "the JPEG scan header names component {id}, which the frame header does not have"
            )));
        };
        named.push(id);
        blocks += u32::from(horizontal) * u32::from(vertical);
    }
    if count > 1 && blocks > 10 {
        return Err(malformed(format!(
            "the JPEG scan interleaves {blocks} blocks in each minimum coded unit, not at \
             most 10"
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
            b"IDAT" => compressed.extend_from_slice(body),
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
                data: compressed,
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
                    assert_eq!(*data, compress_to_vec_zlib(&png.rows(), 6));
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
    /// marker `sof`, and padding before a marker.
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
        data.extend_from_slice(b"\xFF\xC4\x00\x03\x00\xFF\xDA\x00\x08\x01\x01\x00\x00\x3F\x00");
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
                        Encoding::Jpeg { components: c, adobe: a } if c == components && a == adobe
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
                adobe: false
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
                "the JPEG scan header names component 1 twice",
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
            let err = error(data);
            assert!(
                err.starts_with("image data is damaged: ") && err.contains(message),
                "{err}"
            );
        }
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
