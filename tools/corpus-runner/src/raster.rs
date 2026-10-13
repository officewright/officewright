//! Grey page images: the reference pages, 8-bit grey PNG files as `scripts/corpus-import.py`
//! writes them, and our pages, PGM files from `pdftoppm -gray`.

/// The largest image either reader accepts, in pixels: a page at 110 dpi is about a
/// million, so this leaves room for any paper size and refuses absurd headers.
const MAX_PIXELS: usize = 64 * 1024 * 1024;

/// An 8-bit grey image, rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Grey {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Grey {
    /// The pixel at (x, y); white outside the image, which is how pages of different sizes
    /// are compared.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x >= self.width {
            return 255;
        }
        self.pixels.get(y * self.width + x).copied().unwrap_or(255)
    }
}

fn checked_size(width: usize, height: usize) -> Result<usize, String> {
    match width.checked_mul(height) {
        Some(pixels) if width > 0 && height > 0 && pixels <= MAX_PIXELS => Ok(pixels),
        _ => Err(format!("image size {width}x{height} is not accepted")),
    }
}

/// Reads an 8-bit grey, non-interlaced PNG file, the only kind the corpus holds.
pub fn read_png(data: &[u8]) -> Result<Grey, String> {
    let mut rest = data
        .strip_prefix(b"\x89PNG\r\n\x1a\n".as_slice())
        .ok_or("not a PNG file")?;
    let mut header = None;
    let mut compressed = Vec::new();
    loop {
        let length = rest
            .get(..4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_be_bytes)
            .ok_or("PNG chunk header is cut off")? as usize;
        let kind = rest.get(4..8).ok_or("PNG chunk header is cut off")?;
        let body = rest.get(8..8 + length).ok_or("PNG chunk is cut off")?;
        match kind {
            b"IHDR" => header = Some(body.to_vec()),
            b"IDAT" => compressed.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        // The CRC is not checked: the zlib stream inside carries its own checksum.
        rest = rest.get(12 + length..).ok_or("PNG chunk is cut off")?;
    }
    let header = header.ok_or("PNG file without IHDR")?;
    let number = |range: std::ops::Range<usize>| {
        header
            .get(range)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_be_bytes)
            .map(|value| value as usize)
            .ok_or("IHDR is cut off")
    };
    let (width, height) = (number(0..4)?, number(4..8)?);
    if header.get(8..13) != Some([8, 0, 0, 0, 0].as_slice()) {
        return Err("only 8-bit grey, non-interlaced PNG files are read".to_owned());
    }
    let pixels = checked_size(width, height)?;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&compressed, pixels + height)
        .map_err(|error| format!("PNG image data: {error:?}"))?;
    if raw.len() != pixels + height {
        return Err("PNG image data has the wrong size".to_owned());
    }
    let mut out = vec![0u8; pixels];
    let mut previous = vec![0u8; width];
    for (row, filtered) in raw.chunks_exact(width + 1).enumerate() {
        let (&filter, line) = filtered.split_first().ok_or("empty PNG row")?;
        let mut current = vec![0u8; width];
        let mut left = 0u8;
        let mut upper_left = 0u8;
        for ((value, &byte), (slot, &up)) in line
            .iter()
            .zip(line.iter())
            .zip(current.iter_mut().zip(previous.iter()))
        {
            let _ = value;
            let predicted = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => nearest_of_three(left, up, upper_left),
                other => return Err(format!("PNG row {row} has filter {other}")),
            };
            *slot = byte.wrapping_add(predicted);
            left = *slot;
            upper_left = up;
        }
        out.get_mut(row * width..(row + 1) * width)
            .ok_or("PNG row out of range")?
            .copy_from_slice(&current);
        previous = current;
    }
    Ok(Grey {
        width,
        height,
        pixels: out,
    })
}

/// PNG filter type 4: of left, up and upper left, the one closest to left + up - upper left.
fn nearest_of_three(a: u8, b: u8, c: u8) -> u8 {
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

/// Reads a binary PGM file (P5) with a maximum value of 255, as `pdftoppm -gray` writes.
pub fn read_pgm(data: &[u8]) -> Result<Grey, String> {
    let mut fields = Vec::new();
    let mut position = 0;
    // Magic number, width, height and maximum value, separated by white space and comments.
    while fields.len() < 4 {
        while let Some(&byte) = data.get(position) {
            if byte == b'#' {
                while data.get(position).is_some_and(|&byte| byte != b'\n') {
                    position += 1;
                }
            } else if byte.is_ascii_whitespace() {
                position += 1;
            } else {
                break;
            }
        }
        let start = position;
        while data
            .get(position)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            position += 1;
        }
        if start == position {
            return Err("PGM header is cut off".to_owned());
        }
        fields.push(
            String::from_utf8_lossy(data.get(start..position).unwrap_or_default()).into_owned(),
        );
    }
    // Exactly one white-space character ends the header.
    position += 1;
    if fields.first().map(String::as_str) != Some("P5")
        || fields.get(3).map(String::as_str) != Some("255")
    {
        return Err("only binary 8-bit PGM files are read".to_owned());
    }
    let parse = |index: usize| {
        fields
            .get(index)
            .and_then(|field| field.parse::<usize>().ok())
            .ok_or("PGM size is not a number")
    };
    let (width, height) = (parse(1)?, parse(2)?);
    let pixels = checked_size(width, height)?;
    let body = data
        .get(position..position + pixels)
        .ok_or("PGM image data is cut off")?;
    Ok(Grey {
        width,
        height,
        pixels: body.to_vec(),
    })
}

/// An 8-bit grey PNG file of the image, as `scripts/corpus-import.py` writes the reference
/// pages (without valid CRCs, which the reader does not check).
#[cfg(test)]
pub fn encode_png(image: &Grey) -> Vec<u8> {
    let mut raw = Vec::new();
    for row in image.pixels.chunks_exact(image.width) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let chunk = |kind: &[u8], body: &[u8]| {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    };
    let mut header = (image.width as u32).to_be_bytes().to_vec();
    header.extend_from_slice(&(image.height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
    file.extend(chunk(b"IHDR", &header));
    file.extend(chunk(
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
    ));
    file.extend(chunk(b"IEND", b""));
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoded_page_reads_back() {
        let image = Grey {
            width: 3,
            height: 2,
            pixels: vec![0, 10, 20, 30, 40, 255],
        };
        assert_eq!(read_png(&encode_png(&image)).unwrap(), image);
    }

    /// A PNG file with the given filter on every row, as a reference encoder would write.
    fn png(width: usize, rows: &[Vec<u8>], filter: u8) -> Vec<u8> {
        let mut raw = Vec::new();
        let mut previous = vec![0u8; width];
        for row in rows {
            raw.push(filter);
            let mut left = 0u8;
            let mut upper_left = 0u8;
            for (x, &value) in row.iter().enumerate() {
                let up = previous[x];
                let predicted = match filter {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                    _ => nearest_of_three(left, up, upper_left),
                };
                raw.push(value.wrapping_sub(predicted));
                left = value;
                upper_left = up;
            }
            previous = row.clone();
        }
        let chunk = |kind: &[u8], body: &[u8]| {
            let mut out = (body.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out
        };
        let mut header = (width as u32).to_be_bytes().to_vec();
        header.extend_from_slice(&(rows.len() as u32).to_be_bytes());
        header.extend_from_slice(&[8, 0, 0, 0, 0]);
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        file.extend(chunk(b"IHDR", &header));
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        let (first, second) = compressed.split_at(compressed.len() / 2);
        file.extend(chunk(b"IDAT", first));
        file.extend(chunk(b"tEXt", b"ignored"));
        file.extend(chunk(b"IDAT", second));
        file.extend(chunk(b"IEND", b""));
        file
    }

    fn sample() -> Vec<Vec<u8>> {
        vec![
            vec![0, 50, 200, 255, 7],
            vec![10, 60, 190, 250, 9],
            vec![255, 0, 128, 3, 99],
        ]
    }

    #[test]
    fn every_png_filter_is_undone() {
        for filter in 0..=4 {
            let image = read_png(&png(5, &sample(), filter)).unwrap();
            assert_eq!((image.width, image.height), (5, 3), "filter {filter}");
            assert_eq!(image.pixels, sample().concat(), "filter {filter}");
        }
    }

    #[test]
    fn other_pngs_are_refused() {
        let mut colour = png(5, &sample(), 0);
        colour[8 + 8 + 9] = 2; // colour type RGB in IHDR
        assert!(read_png(&colour).is_err());
        let file = png(5, &sample(), 0);
        assert!(read_png(&file[..file.len() - 20]).is_err());
        assert!(read_png(b"GIF89a").is_err());
        let mut bad_filter = sample();
        bad_filter[0][0] = 1;
        let mut file = png(5, &bad_filter, 0);
        // A row filter of 9 cannot be undone.
        let raw = vec![vec![9u8, 1, 2, 3, 4, 5]; 3].concat();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        let start = 8 + 25;
        file.truncate(start);
        file.extend((compressed.len() as u32).to_be_bytes());
        file.extend(b"IDAT");
        file.extend(&compressed);
        file.extend([0, 0, 0, 0, 0, 0, 0, 0]);
        file.extend(b"IEND");
        file.extend([0, 0, 0, 0]);
        assert_eq!(read_png(&file).unwrap_err(), "PNG row 0 has filter 9");
    }

    #[test]
    fn pgm_files_are_read_with_comments() {
        let image = read_pgm(b"P5\n# made by a test\n3 2\n255\n\x00\x01\x02\x03\x04\x05").unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(image.pixels, [0, 1, 2, 3, 4, 5]);
        assert_eq!(image.at(2, 1), 5);
        assert_eq!(image.at(3, 0), 255, "outside is white");
        assert_eq!(image.at(0, 2), 255, "outside is white");
        assert!(read_pgm(b"P5\n3 2\n255\n\x00\x01").is_err());
        assert!(read_pgm(b"P2\n3 2\n255\n0 1 2 3 4 5").is_err());
        assert!(read_pgm(b"P5\n3 2\n65535\n").is_err());
        assert!(read_pgm(b"P5\n99999 99999\n255\n").is_err());
    }
}
