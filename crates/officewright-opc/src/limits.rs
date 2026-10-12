/// Bounds on what reading a package may consume.
///
/// The defaults are far above what real documents need and low enough that a hostile
/// file cannot exhaust memory: across the Apache POI and LibreOffice test documents,
/// packages have at most about 1,700 entries, part names of 81 bytes, 1,500
/// relationships and 500 content types,
/// XML nests at most about 50 elements deep, no element has more than 25 attributes and
/// no Markup Compatibility attribute lists more than about 10 names.
///
/// ```
/// use officewright_opc::Limits;
///
/// let limits = Limits::new().with_max_part_size(64 << 20);
/// assert_eq!(limits.max_part_size(), 64 << 20);
/// assert_eq!(limits.max_xml_depth(), Limits::new().max_xml_depth());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    entries: u64,
    part_size: u64,
    part_name_length: usize,
    relationships: usize,
    content_types: usize,
    total_size: u64,
    xml_depth: usize,
    xml_attributes: usize,
    mce_names: usize,
}

impl Limits {
    /// The default limits: 65,536 ZIP entries, 256 MiB per part, 1 GiB for all parts
    /// read from one package, part names of 1,024 bytes, 262,144 relationships, 65,536
    /// content types, XML nesting 256 elements deep, 256 attributes per element and 256
    /// names per Markup Compatibility attribute.
    pub const fn new() -> Self {
        Self {
            entries: 1 << 16,
            part_size: 256 << 20,
            part_name_length: 1024,
            relationships: 1 << 18,
            content_types: 1 << 16,
            total_size: 1 << 30,
            xml_depth: 256,
            xml_attributes: 256,
            mce_names: 256,
        }
    }

    /// The most entries a ZIP archive may have.
    pub const fn max_entries(&self) -> u64 {
        self.entries
    }

    /// Sets [`max_entries`](Self::max_entries).
    #[must_use]
    pub const fn with_max_entries(mut self, value: u64) -> Self {
        self.entries = value;
        self
    }

    /// The largest uncompressed size of one part, in bytes.
    pub const fn max_part_size(&self) -> u64 {
        self.part_size
    }

    /// Sets [`max_part_size`](Self::max_part_size).
    #[must_use]
    pub const fn with_max_part_size(mut self, value: u64) -> Self {
        self.part_size = value;
        self
    }

    /// The largest total uncompressed size of all parts read from one package, in
    /// bytes. Parts that are never read, such as images when only text is needed, do
    /// not count.
    pub const fn max_total_size(&self) -> u64 {
        self.total_size
    }

    /// Sets [`max_total_size`](Self::max_total_size).
    #[must_use]
    pub const fn with_max_total_size(mut self, value: u64) -> Self {
        self.total_size = value;
        self
    }

    /// The longest part name, in bytes, as written in the ZIP archive or in an
    /// `Override`. Relationship targets are resolved against the folder of their part,
    /// so this also bounds the work of resolving one.
    pub const fn max_part_name_length(&self) -> usize {
        self.part_name_length
    }

    /// Sets [`max_part_name_length`](Self::max_part_name_length).
    #[must_use]
    pub const fn with_max_part_name_length(mut self, value: usize) -> Self {
        self.part_name_length = value;
        self
    }

    /// The most relationships one package may have, over all its relationships parts.
    /// Each takes memory beyond the XML it comes from, so this bound, not only the part
    /// size, keeps opening a package within a predictable amount of memory.
    pub const fn max_relationships(&self) -> usize {
        self.relationships
    }

    /// Sets [`max_relationships`](Self::max_relationships).
    #[must_use]
    pub const fn with_max_relationships(mut self, value: usize) -> Self {
        self.relationships = value;
        self
    }

    /// The most `Default` and `Override` elements `[Content_Types].xml` may have.
    pub const fn max_content_types(&self) -> usize {
        self.content_types
    }

    /// Sets [`max_content_types`](Self::max_content_types).
    #[must_use]
    pub const fn with_max_content_types(mut self, value: usize) -> Self {
        self.content_types = value;
        self
    }

    /// How deeply XML elements may nest; the root element is at depth 1.
    pub const fn max_xml_depth(&self) -> usize {
        self.xml_depth
    }

    /// Sets [`max_xml_depth`](Self::max_xml_depth).
    #[must_use]
    pub const fn with_max_xml_depth(mut self, value: usize) -> Self {
        self.xml_depth = value;
        self
    }

    /// The most attributes one XML element may have, namespace declarations included.
    pub const fn max_xml_attributes(&self) -> usize {
        self.xml_attributes
    }

    /// Sets [`max_xml_attributes`](Self::max_xml_attributes).
    #[must_use]
    pub const fn with_max_xml_attributes(mut self, value: usize) -> Self {
        self.xml_attributes = value;
        self
    }

    /// The most names one Markup Compatibility attribute may list: the prefixes of
    /// `mc:Ignorable`, `mc:MustUnderstand` and `Requires`, the names of
    /// `mc:ProcessContent`, `mc:PreserveElements` and `mc:PreserveAttributes`.
    pub const fn max_mce_names(&self) -> usize {
        self.mce_names
    }

    /// Sets [`max_mce_names`](Self::max_mce_names).
    #[must_use]
    pub const fn with_max_mce_names(mut self, value: usize) -> Self {
        self.mce_names = value;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}
