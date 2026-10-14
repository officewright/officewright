//! Stream objects and their compression.

use miniz_oxide::deflate::compress_to_vec_zlib;

/// How a [`Document`](crate::Document) stores the data of its streams: the content of
/// its pages, the programs and ToUnicode maps of its embedded fonts, and the samples and
/// soft masks of images decoded from PNG files. JPEG files, and the data of PNG files
/// that go into the PDF as they are, keep their own compression either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Compression {
    /// Compressed with the Flate filter (zlib, ISO 32000-1, 7.4.4) wherever that makes a
    /// stream object smaller, the `/Filter` entry and the digits of `/Length` counted;
    /// streams of a few bytes stay as they are. Every PDF reader decompresses Flate, so
    /// this is the default.
    #[default]
    Flate,
    /// Stored as they are, so the page content can be read in a text editor.
    None,
}

/// zlib's own default level: close to the smallest output, at a fraction of the time of
/// the highest level.
const LEVEL: u8 = 6;

/// What compressed data adds to a stream's dictionary.
const FLATE_FILTER: &str = " /Filter /FlateDecode";

/// The digits of `n` in decimal, as `/Length` writes it.
fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |log| log as usize + 1)
}

/// The dictionary and data of a stream object, without the object number: `/Length`,
/// `/Filter /FlateDecode` if the data was compressed, then `entries`, each of which
/// starts with a space.
pub(crate) fn stream_object(entries: &str, data: &[u8], compression: Compression) -> Vec<u8> {
    let compressed = match compression {
        // Kept only when the whole object gets smaller: the filter entry and the digits of
        // `/Length` count too.
        Compression::Flate => Some(compress_to_vec_zlib(data, LEVEL)).filter(|compressed| {
            compressed.len() + FLATE_FILTER.len() + digits(compressed.len())
                < data.len() + digits(data.len())
        }),
        Compression::None => None,
    };
    let (filter, data) = match &compressed {
        Some(compressed) => (FLATE_FILTER, compressed.as_slice()),
        None => ("", data),
    };
    let mut object =
        format!("<< /Length {}{filter}{entries} >>\nstream\n", data.len()).into_bytes();
    object.extend_from_slice(data);
    // The end-of-line before `endstream` is not part of /Length (ISO 32000-1, 7.3.8.1).
    object.extend_from_slice(b"\nendstream");
    object
}

#[cfg(test)]
mod tests {
    use miniz_oxide::inflate::decompress_to_vec_zlib;

    use super::*;

    /// The dictionary and the data of a stream object.
    fn split(object: &[u8]) -> (String, &[u8]) {
        let text = String::from_utf8_lossy(object);
        let (dictionary, _) = text.split_once("\nstream\n").unwrap();
        let start = dictionary.len() + "\nstream\n".len();
        let end = object.len() - "\nendstream".len();
        assert!(object[end..].starts_with(b"\nendstream"));
        (dictionary.to_owned(), &object[start..end])
    }

    #[test]
    fn compresses_data_that_shrinks() {
        let data = "BT\n/F1 12 Tf\n72 700 Td\n(Hello, world) Tj\nET\n".repeat(20);
        let object = stream_object(" /Length1 7", data.as_bytes(), Compression::Flate);
        let (dictionary, stored) = split(&object);
        assert_eq!(
            dictionary,
            format!(
                "<< /Length {} /Filter /FlateDecode /Length1 7 >>",
                stored.len()
            )
        );
        assert!(stored.len() < data.len() / 4);
        assert_eq!(decompress_to_vec_zlib(stored).unwrap(), data.as_bytes());
    }

    #[test]
    fn stores_data_that_would_grow() {
        let data = b"BT (a) Tj ET";
        let object = stream_object("", data, Compression::Flate);
        assert_eq!(object, b"<< /Length 12 >>\nstream\nBT (a) Tj ET\nendstream");
        // Including no data at all.
        let object = stream_object("", b"", Compression::Flate);
        assert_eq!(object, b"<< /Length 0 >>\nstream\n\nendstream");
    }

    #[test]
    fn compresses_exactly_when_the_whole_object_gets_smaller() {
        // Data that hardly compresses: noise, then a run of zeros that grows by a byte at
        // a time, so that the zlib stream gets shorter than the data by about a byte more
        // each time. The noise is 40, 920 and 2000 bytes long: with 40 and 920 the choice
        // turns where the data passes 100 and 1000 bytes and `/Length` gains a digit.
        let mut state: u32 = 1;
        let noise: Vec<u8> = (0..2000)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) as u8
            })
            .collect();
        let mut stored_though_shorter = 0;
        let mut compressed = 0;
        for noise_len in [40, 920, 2000] {
            for zeros in 0..150 {
                let data = [&noise[..noise_len], &vec![0; zeros]].concat();
                let zlib = compress_to_vec_zlib(&data, LEVEL);
                let stored = stream_object("", &data, Compression::None);
                let mut flate = format!(
                    "<< /Length {} /Filter /FlateDecode >>\nstream\n",
                    zlib.len()
                )
                .into_bytes();
                flate.extend_from_slice(&zlib);
                flate.extend_from_slice(b"\nendstream");
                // The smaller of the two; at the same size, the data is stored, so there
                // is nothing to decompress.
                let expected = if flate.len() < stored.len() {
                    flate
                } else {
                    stored
                };
                let object = stream_object("", &data, Compression::Flate);
                assert!(object == expected, "{noise_len} + {zeros} bytes");
                if String::from_utf8_lossy(&object).contains("/Filter") {
                    compressed += 1;
                } else if zlib.len() < data.len() {
                    stored_though_shorter += 1;
                }
            }
        }
        assert!(
            stored_though_shorter > 0 && compressed > 0,
            "{stored_though_shorter} {compressed}"
        );
        // Across 100 bytes: 40 bytes of noise and 60 zeros compress to 80 bytes, which
        // with the filter entry and two digits of `/Length` is as long as the data with
        // three, so the data is stored; with 61 zeros compressing saves a byte.
        for (zeros, filtered) in [(60, false), (61, true)] {
            let data = [&noise[..40], &vec![0; zeros]].concat();
            assert_eq!(compress_to_vec_zlib(&data, LEVEL).len(), 80);
            let object = stream_object("", &data, Compression::Flate);
            assert_eq!(
                String::from_utf8_lossy(&object).contains("/Filter"),
                filtered,
                "{zeros} zeros"
            );
        }
    }

    #[test]
    fn counts_the_digits_of_lengths() {
        assert_eq!(
            [0, 9, 10, 99, 100, 999, 1000, 12_345].map(digits),
            [1, 1, 2, 2, 3, 3, 4, 5]
        );
    }

    #[test]
    fn stores_data_as_it_is_without_compression() {
        let data = "0 0 Td\n".repeat(100);
        let object = stream_object("", data.as_bytes(), Compression::None);
        let (dictionary, stored) = split(&object);
        assert_eq!(dictionary, "<< /Length 700 >>");
        assert_eq!(stored, data.as_bytes());
    }
}
