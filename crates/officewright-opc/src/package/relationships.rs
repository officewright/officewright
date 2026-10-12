use std::collections::HashSet;

use super::part_name::resolve_target;
use super::{PartName, read_flat_xml};
use crate::{Limits, Result};

const NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// The relationships of a part or of the package, from a relationships part such as
/// `/word/_rels/document.xml.rels`.
#[derive(Clone, Debug, Default)]
pub struct Relationships {
    items: Vec<Relationship>,
}

/// A link from a part, or from the package, to a part or to an external resource.
#[derive(Clone, Debug)]
pub struct Relationship {
    id: String,
    relationship_type: String,
    target: String,
    target_mode: TargetMode,
    target_part: Option<PartName>,
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
    pub(crate) fn parse(bytes: &[u8], limits: &Limits, part: &str, folder: &str) -> Result<Self> {
        let mut items = Vec::new();
        let mut ids = HashSet::new();
        read_flat_xml(
            bytes,
            limits,
            part,
            NAMESPACE,
            "Relationships",
            |reader, tag| {
                let error = |message: String| reader.error(tag.offset(), message);
                if tag.name().local() != "Relationship" {
                    return Err(error(format!(
                        "unexpected element <{}>",
                        tag.name().qname().as_str()
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
                                _ => return Err(error(format!("invalid TargetMode {value:?}"))),
                            };
                        }
                        _ => {
                            return Err(error(format!(
                                "unexpected attribute {} on <Relationship>",
                                attribute.name().qname().as_str()
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
                if !id.is_empty() && !ids.insert(id.to_owned()) {
                    return Err(error(format!("a second relationship with Id {id}")));
                }
                items.push(Relationship {
                    id: id.to_owned(),
                    relationship_type: relationship_type.to_owned(),
                    target: target.to_owned(),
                    target_mode,
                    target_part: match target_mode {
                        TargetMode::Internal => resolve_target(folder, target),
                        TargetMode::External => None,
                    },
                });
                Ok(())
            },
        )?;
        Ok(Self { items })
    }

    /// The relationship with this Id.
    pub fn get(&self, id: &str) -> Option<&Relationship> {
        if id.is_empty() {
            return None;
        }
        self.items.iter().find(|relationship| relationship.id == id)
    }

    /// The relationships of one type, such as
    /// `http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles`.
    pub fn of_type<'r>(
        &'r self,
        relationship_type: &'r str,
    ) -> impl Iterator<Item = &'r Relationship> {
        self.items
            .iter()
            .filter(move |relationship| relationship.relationship_type == relationship_type)
    }

    /// All relationships in document order.
    pub fn iter(&self) -> std::slice::Iter<'_, Relationship> {
        self.items.iter()
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
    type IntoIter = std::slice::Iter<'r, Relationship>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
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

    /// The part an internal target names, resolved against the source part. `None` for
    /// external targets and for internal ones that cannot name a part, such as targets
    /// that climb out of the package. The part may still be missing from the package.
    pub fn target_part(&self) -> Option<&PartName> {
        self.target_part.as_ref()
    }
}
