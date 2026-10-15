use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::{Document, Error, Limit, Result};

/// Bounds on what one document may make the reader hold, so that a hostile or damaged
/// file is refused with [`Error::LimitExceeded`] instead of exhausting memory or time.
///
/// The defaults are far above real documents. Build other limits from them:
///
/// ```
/// use officewright_doc::{Limits, LoadOptions};
///
/// let options = LoadOptions::new().with_limits(Limits::new().with_max_file_size(16 << 20));
/// # let _ = options;
/// ```
#[derive(Clone, Debug)]
pub struct Limits {
    package: officewright_opc::Limits,
    file_size: u64,
    nodes: usize,
}

impl Limits {
    /// The default limits: files of up to 1 GiB, 65,536 ZIP entries, 256 MiB per part
    /// and 1 GiB for all parts unpacked, XML nested at most 256 elements deep, and
    /// 8,388,608 elements in the document.
    pub const fn new() -> Self {
        Self {
            package: officewright_opc::Limits::new(),
            file_size: 1 << 30,
            nodes: 1 << 23,
        }
    }

    /// The largest file, in bytes, that [`LoadOptions::open`] and
    /// [`LoadOptions::open_reader`] read, and [`LoadOptions::open_bytes`] accepts.
    pub const fn max_file_size(&self) -> u64 {
        self.file_size
    }

    /// Sets [`max_file_size`](Self::max_file_size).
    #[must_use]
    pub const fn with_max_file_size(mut self, value: u64) -> Self {
        self.file_size = value;
        self
    }

    /// The most entries the ZIP archive may have.
    pub const fn max_entries(&self) -> u64 {
        self.package.max_entries()
    }

    /// Sets [`max_entries`](Self::max_entries).
    #[must_use]
    pub const fn with_max_entries(mut self, value: u64) -> Self {
        self.package = self.package.with_max_entries(value);
        self
    }

    /// The largest size, in bytes, of one part once unpacked.
    pub const fn max_part_size(&self) -> u64 {
        self.package.max_part_size()
    }

    /// Sets [`max_part_size`](Self::max_part_size).
    #[must_use]
    pub const fn with_max_part_size(mut self, value: u64) -> Self {
        self.package = self.package.with_max_part_size(value);
        self
    }

    /// The largest size, in bytes, of all parts read from one package once unpacked.
    pub const fn max_total_size(&self) -> u64 {
        self.package.max_total_size()
    }

    /// Sets [`max_total_size`](Self::max_total_size).
    #[must_use]
    pub const fn with_max_total_size(mut self, value: u64) -> Self {
        self.package = self.package.with_max_total_size(value);
        self
    }

    /// The deepest nesting of XML elements in a part. Tables in tables nest deepest in
    /// real documents.
    pub const fn max_xml_depth(&self) -> usize {
        self.package.max_xml_depth()
    }

    /// Sets [`max_xml_depth`](Self::max_xml_depth).
    #[must_use]
    pub const fn with_max_xml_depth(mut self, value: usize) -> Self {
        self.package = self.package.with_max_xml_depth(value);
        self
    }

    /// The most elements the document may have: paragraphs, runs, what runs contain,
    /// and every element kept without being understood. Each costs more memory than
    /// the few bytes of XML it can come from, so this bound, not only the part size,
    /// keeps a document within a predictable amount of memory.
    pub const fn max_nodes(&self) -> usize {
        self.nodes
    }

    /// Sets [`max_nodes`](Self::max_nodes). Nodes are counted in 32 bits: a value above
    /// 4,294,967,295 is read as that.
    #[must_use]
    pub const fn with_max_nodes(mut self, value: usize) -> Self {
        self.nodes = if value > u32::MAX as usize {
            u32::MAX as usize
        } else {
            value
        };
        self
    }

    /// The longest part name, in bytes, the package may have.
    pub const fn max_part_name_length(&self) -> usize {
        self.package.max_part_name_length()
    }

    /// Sets [`max_part_name_length`](Self::max_part_name_length).
    #[must_use]
    pub const fn with_max_part_name_length(mut self, value: usize) -> Self {
        self.package = self.package.with_max_part_name_length(value);
        self
    }

    /// The most relationships the package may have, over all its relationships parts.
    pub const fn max_relationships(&self) -> usize {
        self.package.max_relationships()
    }

    /// Sets [`max_relationships`](Self::max_relationships).
    #[must_use]
    pub const fn with_max_relationships(mut self, value: usize) -> Self {
        self.package = self.package.with_max_relationships(value);
        self
    }

    /// The most `Default` and `Override` elements `[Content_Types].xml` may have.
    pub const fn max_content_types(&self) -> usize {
        self.package.max_content_types()
    }

    /// Sets [`max_content_types`](Self::max_content_types).
    #[must_use]
    pub const fn with_max_content_types(mut self, value: usize) -> Self {
        self.package = self.package.with_max_content_types(value);
        self
    }

    /// The most attributes one XML element may have.
    pub const fn max_xml_attributes(&self) -> usize {
        self.package.max_xml_attributes()
    }

    /// Sets [`max_xml_attributes`](Self::max_xml_attributes).
    #[must_use]
    pub const fn with_max_xml_attributes(mut self, value: usize) -> Self {
        self.package = self.package.with_max_xml_attributes(value);
        self
    }

    /// The most distinct namespace URIs one XML part may declare: 4,096 by default, where
    /// real parts declare at most about 50.
    pub const fn max_xml_namespaces(&self) -> usize {
        self.package.max_xml_namespaces()
    }

    /// Sets [`max_xml_namespaces`](Self::max_xml_namespaces).
    #[must_use]
    pub const fn with_max_xml_namespaces(mut self, value: usize) -> Self {
        self.package = self.package.with_max_xml_namespaces(value);
        self
    }

    /// The most names one Markup Compatibility attribute (such as `mc:Ignorable`) may
    /// list.
    pub const fn max_mce_names(&self) -> usize {
        self.package.max_mce_names()
    }

    /// Sets [`max_mce_names`](Self::max_mce_names).
    #[must_use]
    pub const fn with_max_mce_names(mut self, value: usize) -> Self {
        self.package = self.package.with_max_mce_names(value);
        self
    }

    pub(crate) const fn package(&self) -> &officewright_opc::Limits {
        &self.package
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

/// How to open a document.
///
/// [`Document::open`], [`Document::from_bytes`] and [`Document::from_reader`] use the
/// default options; these methods take others.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    limits: Limits,
}

impl LoadOptions {
    /// The default options.
    pub const fn new() -> Self {
        Self {
            limits: Limits::new(),
        }
    }

    /// The limits documents are opened with.
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Sets [`limits`](Self::limits).
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Opens the document in a file.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the file cannot be read, and the errors of
    /// [`open_bytes`](Self::open_bytes).
    pub fn open(&self, path: impl AsRef<Path>) -> Result<Document> {
        let path = path.as_ref();
        let io = |source| Error::Io {
            message: format!("cannot read {}", path.display()),
            source,
        };
        self.open_reader_named(File::open(path).map_err(io)?, &path.display().to_string())
    }

    /// Opens the document in `data`.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] when the data is not a WordprocessingML package or its
    /// structure is broken, [`Error::Unsupported`] for an encrypted or binary Word
    /// document, and [`Error::LimitExceeded`] when it exceeds the [`Limits`].
    pub fn open_bytes(&self, data: impl Into<Vec<u8>>) -> Result<Document> {
        let data = data.into();
        if data.len() as u64 > self.limits.file_size {
            return Err(Error::LimitExceeded {
                limit: Limit::FileSize,
                maximum: self.limits.file_size,
                context: "the input".into(),
            });
        }
        crate::docx::read(data, &self.limits)
    }

    /// Reads a document to its end and opens it.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when reading fails, and the errors of
    /// [`open_bytes`](Self::open_bytes).
    pub fn open_reader(&self, reader: impl Read) -> Result<Document> {
        self.open_reader_named(reader, "the input")
    }

    fn open_reader_named(&self, reader: impl Read, name: &str) -> Result<Document> {
        let mut data = Vec::new();
        reader
            .take(self.limits.file_size.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|source| Error::Io {
                message: format!("cannot read {name}"),
                source,
            })?;
        if data.len() as u64 > self.limits.file_size {
            return Err(Error::LimitExceeded {
                limit: Limit::FileSize,
                maximum: self.limits.file_size,
                context: name.to_owned(),
            });
        }
        crate::docx::read(data, &self.limits)
    }
}
