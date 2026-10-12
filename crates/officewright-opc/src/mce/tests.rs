use super::*;
use crate::Limits;
use crate::xml::{Event, Input};

const DECLARATIONS: &str = r#"xmlns:w="urn:w" xmlns:a="urn:a" xmlns:b="urn:b" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

/// For every start tag: `local=Disposition`, then `@attribute=Disposition` for each
/// attribute, and `Choice=selectable` for `mc:Choice`.
fn walk(body: &str, understood: &[&str]) -> Result<Vec<String>> {
    let xml = format!("<w:root {DECLARATIONS}>{body}</w:root>");
    let input = Input::decode(xml.as_bytes())?;
    let mut reader = input.reader(&Limits::new());
    let mut mce = Processor::new(&mut reader);
    for uri in understood {
        let namespace = reader.intern(uri);
        mce.understand(namespace);
    }
    let mut out = Vec::new();
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => {
                mce.start(&reader, &tag)?;
                if tag.name().local() == "root" {
                    continue;
                }
                out.push(format!(
                    "{}={:?}",
                    tag.name().local(),
                    mce.element(tag.name())
                ));
                for attribute in tag.attributes() {
                    out.push(format!(
                        "@{}={:?}",
                        attribute.name().local(),
                        mce.attribute(attribute.name())
                    ));
                }
                if tag.name().local() == "Choice" {
                    out.push(format!("Choice={}", mce.is_selectable(&reader, &tag)?));
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    Ok(out)
}

fn error(body: &str, understood: &[&str]) -> String {
    match walk(body, understood) {
        Err(err) => err.to_string(),
        Ok(out) => panic!("expected an error, got {out:?}"),
    }
}

#[test]
fn classifies_elements_and_attributes() {
    assert_eq!(
        walk(
            r#"<w:p mc:Ignorable="a" a:id="1" b:id="2" w:val="3" val="4" xml:space="preserve"><a:x/><b:y/><plain/></w:p>"#,
            &["urn:w"]
        )
        .unwrap(),
        [
            "p=Understood",
            "@Ignorable=Understood",
            "@id=Ignore",
            "@id=NotUnderstood",
            "@val=Understood",
            "@val=Understood",
            "@space=Understood",
            "x=Ignore",
            "y=NotUnderstood",
            "plain=NotUnderstood",
        ]
    );
}

#[test]
fn applies_ignorable_to_its_element_and_descendants_only() {
    assert_eq!(
        walk(r#"<a:x mc:Ignorable="a"><a:y/></a:x><a:z/>"#, &["urn:w"]).unwrap(),
        [
            "x=Ignore",
            "@Ignorable=Understood",
            "y=Ignore",
            "z=NotUnderstood"
        ]
    );
}

#[test]
fn processes_the_content_of_listed_elements() {
    assert_eq!(
        walk(
            r#"<w:p mc:Ignorable="a b" mc:ProcessContent="a:wrap b:*"><a:wrap/><a:other/><b:any/></w:p>"#,
            &["urn:w"]
        )
        .unwrap()[3..],
        ["wrap=ProcessContent", "other=Ignore", "any=ProcessContent"]
    );
}

#[test]
fn selects_choices_whose_namespaces_are_understood() {
    let body = r#"<mc:AlternateContent><mc:Choice Requires="a b"/><mc:Choice Requires="a"/><mc:Fallback/></mc:AlternateContent>"#;
    let selectable = |understood: &[&str]| -> Vec<String> {
        walk(body, understood)
            .unwrap()
            .into_iter()
            .filter(|line| line == "Choice=true" || line == "Choice=false")
            .collect()
    };
    assert_eq!(selectable(&["urn:a"]), ["Choice=false", "Choice=true"]);
    assert_eq!(
        selectable(&["urn:a", "urn:b"]),
        ["Choice=true", "Choice=true"]
    );
    assert_eq!(selectable(&[]), ["Choice=false", "Choice=false"]);
}

#[test]
fn rejects_content_that_must_be_understood() {
    assert!(walk(r#"<w:p mc:MustUnderstand="w"/>"#, &["urn:w"]).is_ok());
    assert!(matches!(
        walk(r#"<w:p mc:MustUnderstand="a"/>"#, &["urn:w"]),
        Err(Error::Unsupported(message)) if message.starts_with("the document requires namespace urn:a")
    ));
}

#[test]
fn rejects_malformed_compatibility_attributes() {
    assert_eq!(
        error(r#"<w:p mc:ProcessContent="a"/>"#, &[]),
        "line 1, column 128: ProcessContent lists a, which is not a qualified name"
    );
    assert_eq!(
        error(r#"<w:p mc:PreserveElements=":x"/>"#, &[]),
        "line 1, column 128: PreserveElements lists :x, which is not a qualified name"
    );
    assert_eq!(
        error(r#"<w:p mc:MustUnderstand="z"/>"#, &[]),
        "line 1, column 128: MustUnderstand names the undeclared prefix z"
    );
    assert_eq!(
        error(r#"<w:p mc:Other="a"/>"#, &[]),
        "line 1, column 128: unknown markup compatibility attribute Other"
    );
    assert_eq!(
        error(r#"<mc:Choice/>"#, &[]),
        "line 1, column 128: mc:Choice lacks Requires"
    );
    assert_eq!(
        error(r#"<mc:Choice Requires=" "/>"#, &[]),
        "line 1, column 128: mc:Choice lacks Requires"
    );
    assert_eq!(
        error(r#"<mc:Choice Requires="z"/>"#, &[]),
        "line 1, column 128: Requires names the undeclared prefix z"
    );
}

#[test]
fn skips_undeclared_prefixes_in_lists_of_names() {
    // An undeclared prefix cannot name anything, so it is no reason to reject the part.
    assert_eq!(
        walk(
            r#"<w:p mc:Ignorable="z a" mc:ProcessContent="z:x a:y" mc:PreserveElements="z:x"><a:y/></w:p>"#,
            &["urn:w"]
        )
        .unwrap()[4..],
        ["y=ProcessContent"]
    );
}
