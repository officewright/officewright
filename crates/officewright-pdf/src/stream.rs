//! Stream objects and their compression.

use miniz_oxide::deflate::compress_to_vec_zlib;

/// How a [`Document`](crate::Document) stores the data of its streams: the content of
/// its pages, and the programs and ToUnicode maps of its embedded fonts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Compression {
    /// Compressed with the Flate filter (zlib, ISO 32000-1, 7.4.4) wherever that makes a
    /// stream smaller; streams of a few bytes stay as they are. Every PDF reader
    /// decompresses Flate, so this is the default.
    #[default]
    Flate,
    /// Stored as they are, so the page content can be read in a text editor.
    None,
}

/// zlib's own default level: close to the smallest output, at a fraction of the time of
/// the highest level.
const LEVEL: u8 = 6;

/// The dictionary and data of a stream object, without the object number: `/Length`,
/// `/Filter /FlateDecode` if the data was compressed, then `entries`, each of which
/// starts with a space.
pub(crate) fn stream_object(entries: &str, data: &[u8], compression: Compression) -> Vec<u8> {
    let compressed = match compression {
        Compression::Flate => Some(compress_to_vec_zlib(data, LEVEL))
            .filter(|compressed| compressed.len() < data.len()),
        Compression::None => None,
    };
    let (filter, data) = match &compressed {
        Some(compressed) => (" /Filter /FlateDecode", compressed.as_slice()),
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
    fn stores_data_as_it_is_without_compression() {
        let data = "0 0 Td\n".repeat(100);
        let object = stream_object("", data.as_bytes(), Compression::None);
        let (dictionary, stored) = split(&object);
        assert_eq!(dictionary, "<< /Length 700 >>");
        assert_eq!(stored, data.as_bytes());
    }
}
