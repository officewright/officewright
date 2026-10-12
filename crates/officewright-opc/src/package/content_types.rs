use std::collections::HashMap;
use std::sync::Arc;

use super::{PartName, check_part_name_length, read_flat_xml, shorten};
use crate::{Error, Limit, Limits, Result};

const NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// The content type stream, `[Content_Types].xml`: content types by extension and by
/// part name (ECMA-376 Part 2).
#[derive(Debug, Default)]
pub(crate) struct ContentTypes {
    /// By extension in ASCII lower case. Equal content types share one allocation, and
    /// parts share it too, so a long content type costs its length once.
    defaults: HashMap<String, Arc<str>>,
    /// By [`PartName::key`].
    overrides: HashMap<String, Arc<str>>,
}

impl ContentTypes {
    pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut types = Self::default();
        let mut count = 0;
        let mut interned: HashMap<String, Arc<str>> = HashMap::new();
        read_flat_xml(
            bytes,
            limits,
            "[Content_Types].xml",
            NAMESPACE,
            "Types",
            |reader, tag| {
                count += 1;
                if count > limits.max_content_types() {
                    return Err(Error::LimitExceeded {
                        limit: Limit::ContentTypes,
                        maximum: limits.max_content_types() as u64,
                        context: "[Content_Types].xml".into(),
                    });
                }
                let error = |message: String| reader.error(tag.offset(), message);
                let attribute = |name: &str| {
                    tag.attribute(None, name).ok_or_else(|| {
                        error(format!(
                            "<{}> lacks the {name} attribute",
                            tag.name().qname().as_str()
                        ))
                    })
                };
                let (key, content_type, map) = match tag.name().local() {
                    "Default" => {
                        let extension = attribute("Extension")?;
                        if extension.is_empty() || extension.contains(['.', '/', '\\']) {
                            return Err(error(format!("invalid extension {extension:?}")));
                        }
                        (
                            extension.to_ascii_lowercase(),
                            attribute("ContentType")?,
                            &mut types.defaults,
                        )
                    }
                    "Override" => {
                        let name = attribute("PartName")?;
                        check_part_name_length(name, limits)?;
                        let name = PartName::new(name).map_err(|err| error(err.to_string()))?;
                        (
                            name.key().to_owned(),
                            attribute("ContentType")?,
                            &mut types.overrides,
                        )
                    }
                    other => return Err(error(format!("unexpected element <{other}>"))),
                };
                if tag.attributes().len() != 2 {
                    return Err(error(format!(
                        "unexpected attributes on <{}>",
                        tag.name().qname().as_str()
                    )));
                }
                if !is_media_type(content_type) {
                    return Err(error(format!(
                        "invalid content type {:?}",
                        shorten(content_type, 60)
                    )));
                }
                let shared = Arc::clone(
                    interned
                        .entry(content_type.to_owned())
                        .or_insert_with(|| content_type.into()),
                );
                if map.insert(key, shared).is_some() {
                    return Err(error(format!(
                        "a second <{}> for the same {}",
                        tag.name().local(),
                        if tag.name().local() == "Default" {
                            "extension"
                        } else {
                            "part"
                        }
                    )));
                }
                Ok(())
            },
        )?;
        Ok(types)
    }

    /// The content type of a part: its override, or else the default for its extension.
    pub(crate) fn get(&self, part: &PartName) -> Option<&Arc<str>> {
        self.overrides
            .get(part.key())
            .or_else(|| self.defaults.get(&part.extension()?.to_ascii_lowercase()))
    }
}

/// `type "/" subtype` followed by optional parameters (RFC 7231, section 3.1.1.1), with
/// type and subtype at most 127 characters long (RFC 6838, section 4.2).
fn is_media_type(value: &str) -> bool {
    let essence = value.split(';').next().unwrap_or_default();
    let is_token = |token: &str| {
        !token.is_empty()
            && token.len() <= 127
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    };
    essence
        .split_once('/')
        .is_some_and(|(kind, subtype)| is_token(kind) && is_token(subtype))
}
