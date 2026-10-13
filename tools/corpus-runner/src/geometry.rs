//! Pages, lines and words with their boxes: from a reference's `geometry.json`, and from
//! `pdftotext -bbox-layout` of our PDF, which is how `scripts/corpus-import.py` made the
//! reference's. Points from the top left of the page, y downwards, rounded to 0.001 pt as the
//! importer rounds them.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub x_min: f64,
    pub y_min: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub pages: Vec<Page>,
}

#[derive(Deserialize)]
struct ReferenceFile {
    format: u32,
    pages: Vec<ReferencePage>,
}

#[derive(Deserialize)]
struct ReferencePage {
    lines: Vec<ReferenceLine>,
}

#[derive(Deserialize)]
struct ReferenceLine {
    words: Vec<ReferenceWord>,
}

#[derive(Deserialize)]
struct ReferenceWord {
    text: String,
    #[serde(rename = "box")]
    bounds: [f64; 4],
}

impl Geometry {
    /// A reference's `geometry.json`.
    pub fn from_reference(json: &str) -> Result<Self, String> {
        let file: ReferenceFile =
            serde_json::from_str(json).map_err(|error| format!("geometry.json: {error}"))?;
        if file.format != 1 {
            return Err(format!("geometry.json has format {}, not 1", file.format));
        }
        let pages = file
            .pages
            .into_iter()
            .map(|page| Page {
                lines: page
                    .lines
                    .into_iter()
                    .map(|line| Line {
                        words: line
                            .words
                            .into_iter()
                            .map(|word| Word {
                                text: word.text,
                                x_min: word.bounds[0],
                                y_min: word.bounds[1],
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Ok(Geometry { pages })
    }

    /// The XHTML of `pdftotext -bbox-layout`: `page`, `line` and `word` elements in reading
    /// order, the word's text as character data.
    pub fn from_bbox(xhtml: &str) -> Result<Self, String> {
        let mut geometry = Geometry::default();
        let mut rest = xhtml;
        while let Some(start) = rest.find('<') {
            rest = rest.get(start + 1..).unwrap_or_default();
            let end = rest
                .find('>')
                .ok_or("pdftotext output: a tag is not closed")?;
            let tag = rest.get(..end).unwrap_or_default();
            rest = rest.get(end + 1..).unwrap_or_default();
            let name = tag.split_whitespace().next().unwrap_or_default();
            match name {
                "page" => geometry.pages.push(Page::default()),
                "line" => geometry
                    .pages
                    .last_mut()
                    .ok_or("pdftotext output: a line outside a page")?
                    .lines
                    .push(Line::default()),
                "word" => {
                    let close = rest
                        .find("</word>")
                        .ok_or("pdftotext output: a word is not closed")?;
                    let text = unescape(rest.get(..close).unwrap_or_default());
                    rest = rest.get(close + "</word>".len()..).unwrap_or_default();
                    let word = Word {
                        text,
                        x_min: attribute(tag, "xMin")?,
                        y_min: attribute(tag, "yMin")?,
                    };
                    geometry
                        .pages
                        .last_mut()
                        .and_then(|page| page.lines.last_mut())
                        .ok_or("pdftotext output: a word outside a line")?
                        .words
                        .push(word);
                }
                _ => {}
            }
        }
        Ok(geometry)
    }
}

/// A number attribute, rounded to 0.001 as the importer rounds it.
fn attribute(tag: &str, name: &str) -> Result<f64, String> {
    let key = format!(" {name}=\"");
    let start = tag
        .find(&key)
        .ok_or_else(|| format!("pdftotext output: a word without {name}"))?
        + key.len();
    let value = tag
        .get(start..)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default();
    let number: f64 = value
        .parse()
        .map_err(|_| format!("pdftotext output: {name}=\"{value}\" is not a number"))?;
    Ok((number * 1000.0).round() / 1000.0)
}

/// The XML entities pdftotext writes, and character references.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(rest.get(..start).unwrap_or_default());
        rest = rest.get(start..).unwrap_or_default();
        let Some(end) = rest.find(';') else { break };
        let entity = rest.get(1..end).unwrap_or_default();
        let character = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        };
        match character {
            Some(character) => {
                out.push(character);
                rest = rest.get(end + 1..).unwrap_or_default();
            }
            None => {
                out.push('&');
                rest = rest.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BBOX: &str = r#"<!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml">
<head><title></title><meta name="Producer" content="x"/></head><body><doc>
  <page width="595.320000" height="841.920000">
    <flow><block xMin="72" yMin="103.82" xMax="338" yMax="114.86">
        <line xMin="72.024000" yMin="103.820000" xMax="338.463360" yMax="114.860000">
          <word xMin="72.024000" yMin="103.820000" xMax="93.011040" yMax="114.860000">Case</word>
          <word xMin="95.4288004" yMin="103.8204999" xMax="104.017920" yMax="114.860000">A&amp;B &lt;1&gt; &#x41;&#66;</word>
        </line>
    </block></flow>
  </page>
  <page width="612" height="792">
    <flow><block xMin="1" yMin="1" xMax="2" yMax="2"><line xMin="1" yMin="1" xMax="2" yMax="2">
      <word xMin="10" yMin="20" xMax="30" yMax="40">Two</word></line></block></flow>
  </page>
</doc></body></html>"#;

    #[test]
    fn pdftotext_output_is_read_in_reading_order() {
        let geometry = Geometry::from_bbox(BBOX).unwrap();
        assert_eq!(geometry.pages.len(), 2);
        let words = &geometry.pages[0].lines[0].words;
        assert_eq!(
            words[0],
            Word {
                text: "Case".into(),
                x_min: 72.024,
                y_min: 103.82
            }
        );
        assert_eq!(words[1].text, "A&B <1> AB");
        assert_eq!((words[1].x_min, words[1].y_min), (95.429, 103.82));
        assert_eq!(geometry.pages[1].lines[0].words[0].text, "Two");
    }

    #[test]
    fn broken_output_is_an_error() {
        assert!(Geometry::from_bbox("<page><line><word xMin=\"1\" yMin=\"2\">x").is_err());
        assert!(Geometry::from_bbox("<page><word xMin=\"1\" yMin=\"2\">x</word>").is_err());
        assert!(Geometry::from_bbox("<page><line><word yMin=\"2\">x</word>").is_err());
        assert!(Geometry::from_bbox("<page><line><word xMin=\"a\" yMin=\"2\">x</word>").is_err());
        assert_eq!(Geometry::from_bbox("no tags").unwrap(), Geometry::default());
    }

    #[test]
    fn unknown_entities_stay() {
        assert_eq!(unescape("a &nbsp; b & c &#xZZ;"), "a &nbsp; b & c &#xZZ;");
    }

    #[test]
    fn a_reference_is_read() {
        let json = r#"{"format": 1, "document": "x.docx", "page_count": 1, "pages": [
            {"number": 1, "width": 595.32, "height": 841.92, "lines": [
                {"text": "Case 1", "box": [72.0, 100.0, 120.0, 111.0], "words": [
                    {"text": "Case", "box": [72.0, 100.0, 95.0, 111.0]},
                    {"text": "1", "box": [97.5, 100.0, 103.0, 111.0]}]}]}]}"#;
        let geometry = Geometry::from_reference(json).unwrap();
        assert_eq!(
            geometry.pages[0].lines[0].words[1],
            Word {
                text: "1".into(),
                x_min: 97.5,
                y_min: 100.0
            }
        );
        assert!(Geometry::from_reference(&json.replace("\"format\": 1", "\"format\": 2")).is_err());
        assert!(Geometry::from_reference("{}").is_err());
    }
}
