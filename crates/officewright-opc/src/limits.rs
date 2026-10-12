/// Bounds on what reading a package may consume.
///
/// The defaults are far above what real documents need and low enough that a hostile
/// file cannot exhaust memory: across the Apache POI and LibreOffice test documents,
/// packages have at most about 1,700 entries, XML nests at most about 50 elements deep
/// and no element has more than 25 attributes.
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
    xml_depth: usize,
    xml_attributes: usize,
}

impl Limits {
    /// The default limits: 65,536 ZIP entries, 256 MiB per part, XML nesting 256
    /// elements deep and 256 attributes per element.
    pub const fn new() -> Self {
        Self {
            entries: 1 << 16,
            part_size: 256 << 20,
            xml_depth: 256,
            xml_attributes: 256,
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
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}
