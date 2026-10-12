use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter::FusedIterator;
use std::sync::Arc;

use super::part_name::resolve_target;
use super::{PartName, read_flat_xml, shorten};
use crate::{Error, Limit, Limits, Result};

const NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// The relationships of a part or of the package, from a relationships part such as
/// `/word/_rels/document.xml.rels`.
#[derive(Clone, Debug, Default)]
pub struct Relationships {
    items: Vec<Relationship>,
    /// Index into `items` by Id, for relationships that have one.
    by_id: HashMap<Box<str>, usize>,
}

/// A link from a part, or from the package, to a part or to an external resource.
///
/// The memory a relationship takes is that of its own attributes: the folder its target
/// is resolved against is shared by all relationships of a part, and so is each
/// relationship type.
#[derive(Clone)]
pub struct Relationship {
    id: String,
    relationship_type: Arc<str>,
    target: String,
    target_mode: TargetMode,
    folder: Arc<Folder>,
}

/// What resolving a target needs from the source part, shared by its relationships.
#[derive(Debug)]
struct Folder {
    /// The folder of the source part, `""` for the package.
    path: Box<str>,
    /// [`Limits::max_part_name_length`].
    max_part_name_length: usize,
}

/// Whether a relationship points into the package or outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetMode {
    /// A part of the package.
    Internal,
    /// A resource outside the package, such as a hyperlink. Nothing here ever loads it.
    External,
}

impl Relationships {
    /// Parses a relationships part; `count` is the number of relationships the
    /// package's other relationships parts had.
    pub(crate) fn parse(
        bytes: &[u8],
        limits: &Limits,
        part: &str,
        folder: &str,
        count: &mut usize,
    ) -> Result<Self> {
        let mut items = Vec::new();
        let mut by_id = HashMap::new();
        let mut types: HashSet<Arc<str>> = HashSet::new();
        let folder = Arc::new(Folder {
            path: folder.into(),
            max_part_name_length: limits.max_part_name_length(),
        });
        read_flat_xml(
            bytes,
            limits,
            part,
            NAMESPACE,
            "Relationships",
            |reader, tag| {
                *count += 1;
                if *count > limits.max_relationships() {
                    return Err(Error::LimitExceeded {
                        limit: Limit::Relationships,
                        maximum: limits.max_relationships() as u64,
                        context: part.to_owned(),
                    });
                }
                let error = |message: String| reader.error(tag.offset(), message);
                if tag.name().local() != "Relationship" {
                    return Err(error(format!(
                        "unexpected element <{}>",
                        shorten(tag.name().qname().as_str(), 64)
                    )));
                }
                let mut id = None;
                let mut relationship_type = None;
                let mut target = None;
                let mut target_mode = TargetMode::Internal;
                for attribute in tag.attributes() {
                    let value = attribute.value();
                    match (attribute.name().namespace(), attribute.name().local()) {
                        (None, "Id") => id = Some(value),
                        (None, "Type") => relationship_type = Some(value),
                        (None, "Target") => target = Some(value),
                        (None, "TargetMode") => {
                            target_mode = match value {
                                "Internal" => TargetMode::Internal,
                                "External" => TargetMode::External,
                                _ => {
                                    return Err(error(format!(
                                        "invalid TargetMode {:?}",
                                        shorten(value, 64)
                                    )));
                                }
                            };
                        }
                        _ => {
                            return Err(error(format!(
                                "unexpected attribute {} on <Relationship>",
                                shorten(attribute.name().qname().as_str(), 64)
                            )));
                        }
                    }
                }
                let missing =
                    |name: &str| error(format!("<Relationship> lacks the {name} attribute"));
                // Without an Id a relationship can still be found by its type, and without a
                // Target it names nothing; POI's test-data/spreadsheet/64450.xlsx has an
                // external relationship with neither.
                let id = id.unwrap_or_default();
                let target = target.unwrap_or_default();
                let relationship_type = relationship_type
                    .filter(|kind| !kind.is_empty())
                    .ok_or_else(|| missing("Type"))?;
                if !id.is_empty() && by_id.insert(id.into(), items.len()).is_some() {
                    return Err(error(format!(
                        "a second relationship with Id {}",
                        shorten(id, 64)
                    )));
                }
                let relationship_type = match types.get(relationship_type) {
                    Some(shared) => Arc::clone(shared),
                    None => {
                        let shared: Arc<str> = relationship_type.into();
                        types.insert(Arc::clone(&shared));
                        shared
                    }
                };
                items.push(Relationship {
                    id: id.to_owned(),
                    relationship_type,
                    target: target.to_owned(),
                    target_mode,
                    folder: Arc::clone(&folder),
                });
                Ok(())
            },
        )?;
        Ok(Self { items, by_id })
    }

    /// The relationship with this Id.
    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.items.get(*self.by_id.get(id)?)
    }

    /// The relationships of one type, such as
    /// `http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles`.
    pub fn of_type<'r>(
        &'r self,
        relationship_type: &'r str,
    ) -> impl Iterator<Item = &'r Relationship> {
        self.items
            .iter()
            .filter(move |relationship| &*relationship.relationship_type == relationship_type)
    }

    /// All relationships in document order.
    pub fn iter(&self) -> RelationshipIter<'_> {
        RelationshipIter(self.items.iter())
    }

    /// How many relationships there are.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<'r> IntoIterator for &'r Relationships {
    type Item = &'r Relationship;
    type IntoIter = RelationshipIter<'r>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The relationships of a [`Relationships`] in document order, from
/// [`Relationships::iter`].
#[derive(Clone, Debug)]
pub struct RelationshipIter<'r>(std::slice::Iter<'r, Relationship>);

impl<'r> Iterator for RelationshipIter<'r> {
    type Item = &'r Relationship;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for RelationshipIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back()
    }
}

impl ExactSizeIterator for RelationshipIter<'_> {}

impl FusedIterator for RelationshipIter<'_> {}

impl fmt::Debug for Relationship {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The folder is shared by all relationships of a part and left out, so that the
        // output grows with the relationships part, not with the relationships times it.
        f.debug_struct("Relationship")
            .field("id", &self.id)
            .field("relationship_type", &self.relationship_type)
            .field("target", &self.target)
            .field("target_mode", &self.target_mode)
            .finish_non_exhaustive()
    }
}

impl Relationship {
    /// The Id, unique among the relationships of its source, such as `rId1`; empty when
    /// the relationship has none.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The relationship type, an absolute URI.
    pub fn relationship_type(&self) -> &str {
        &self.relationship_type
    }

    /// The target as written: a URI reference, relative to the source part for internal
    /// targets.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Whether the target is inside the package.
    pub fn target_mode(&self) -> TargetMode {
        self.target_mode
    }

    /// The part an internal target names, resolved against the source part when asked
    /// for. `None` for external targets and for internal ones that cannot name a part:
    /// targets that climb out of the package or are only a fragment, and targets so long
    /// that the name would be over [`Limits::max_part_name_length`]. The part may still be
    /// missing from the package.
    ///
    /// Each call builds a new name of up to that many bytes, so resolve targets as they
    /// are needed and keep only the names of parts that exist, rather than collecting the
    /// targets of all relationships.
    pub fn target_part(&self) -> Option<PartName> {
        match self.target_mode {
            TargetMode::Internal => resolve_target(
                &self.folder.path,
                &self.target,
                self.folder.max_part_name_length,
            ),
            TargetMode::External => None,
        }
    }
}
