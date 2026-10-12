use super::*;
use crate::xml::{Event, Input};
use crate::{Limit, Limits};

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
                // Every element is passed to start() here, skipped or not, to see what
                // it says about each of them.
                let disposition = mce.start(&reader, &tag)?;
                if tag.name().local() == "root" {
                    continue;
                }
                out.push(format!("{}={disposition:?}", tag.name().local()));
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

/// A document that a reviewer of #4 built: `names` copies of the prefix `b` in
/// `mc:Ignorable`, then `elements` elements in namespace `urn:a`, which is neither
/// understood nor ignorable. With 400,000 names and 1,000,000 elements it is byte for
/// byte their probe file `mce-dos.xml`, on which the processor, scanning a list of all
/// 400,000 names for every element, ran for more than 300 s.
fn repeated_ignorable(names: usize, elements: usize) -> String {
    let mut xml = String::from(
        r#"<w:document xmlns:w="urn:w" xmlns:a="urn:a" xmlns:b="urn:b" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable=""#,
    );
    xml.push_str(&vec!["b"; names].join(" "));
    xml.push_str(r#"">"#);
    xml.push_str(&"<a:x/>".repeat(elements));
    xml.push_str("</w:document>");
    xml
}

/// The longest the probe below may take. It reads in about a second in a debug build;
/// with names kept in a list, as before 3e43585, it takes hours, so this bound is what
/// fails the test rather than a hung test run.
const LINEAR_TIME: std::time::Duration = std::time::Duration::from_secs(30);

/// Reads a document as a consumer does, understanding `urn:w` and skipping everything
/// else; returns how many elements are not understood. Fails once reading has taken
/// longer than [`LINEAR_TIME`].
fn count_not_understood(xml: &str, limits: &Limits) -> Result<usize> {
    let started = std::time::Instant::now();
    let input = Input::decode(xml.as_bytes())?;
    let mut reader = input.reader(limits);
    let mut mce = Processor::new(&mut reader);
    let w = reader.intern("urn:w");
    mce.understand(w);
    let mut not_understood = 0;
    let mut elements = 0_u64;
    while let Some(event) = reader.read_event()? {
        match event {
            Event::Start(tag) => {
                elements += 1;
                if elements % 4096 == 0 {
                    assert!(
                        started.elapsed() < LINEAR_TIME,
                        "{elements} elements took {:?}: reading is no longer linear",
                        started.elapsed()
                    );
                }
                let disposition = mce.start(&reader, &tag)?;
                if disposition == Disposition::NotUnderstood {
                    not_understood += 1;
                }
                if disposition != Disposition::Understood {
                    reader.skip_element()?;
                    mce.end();
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    Ok(not_understood)
}

#[test]
fn refuses_an_attribute_with_more_names_than_the_limit() {
    let probe = repeated_ignorable(400_000, 1_000_000);
    assert_eq!(probe.len(), 6_800_159);
    assert!(matches!(
        count_not_understood(&probe, &Limits::new()),
        Err(Error::LimitExceeded {
            limit: Limit::MceNames,
            maximum: 256,
            ref context,
        }) if context == "line 1, column 1"
    ));
    let at_the_limit = repeated_ignorable(256, 10);
    assert_eq!(
        count_not_understood(&at_the_limit, &Limits::new()).unwrap(),
        10
    );
}

#[test]
fn keeps_each_name_once_so_repeated_names_cost_nothing() {
    // Without the limit the probe must still read in linear time: the repeated prefix is
    // kept once, and an element's namespace is looked up in a set, not in a list.
    let limits = Limits::new().with_max_mce_names(400_000);
    let probe = repeated_ignorable(400_000, 1_000_000);
    assert_eq!(count_not_understood(&probe, &limits).unwrap(), 1_000_000);
}

#[test]
fn keeps_a_name_an_outer_element_declared_after_an_inner_one_ends() {
    assert_eq!(
        walk(
            r#"<w:p mc:Ignorable="a" mc:ProcessContent="a:x"><w:r mc:Ignorable="a" mc:ProcessContent="a:x a:y"/><a:x/><a:y/></w:p>"#,
            &["urn:w"]
        )
        .unwrap(),
        [
            "p=Understood",
            "@Ignorable=Understood",
            "@ProcessContent=Understood",
            "r=Understood",
            "@Ignorable=Understood",
            "@ProcessContent=Understood",
            "x=ProcessContent",
            "y=Ignore",
        ]
    );
}

#[test]
fn limits_every_list_of_names() {
    let limits = Limits::new().with_max_mce_names(2);
    let too_many = [
        r#"<w:p mc:ProcessContent="a:x a:y a:z"/>"#,
        r#"<w:p mc:PreserveElements="a:x a:y a:z"/>"#,
        r#"<w:p mc:MustUnderstand="w w w"/>"#,
        r#"<mc:AlternateContent><mc:Choice Requires="a b a"/></mc:AlternateContent>"#,
    ];
    for body in too_many {
        let xml = format!("<w:root {DECLARATIONS}>{body}</w:root>");
        let input = Input::decode(xml.as_bytes()).unwrap();
        let mut reader = input.reader(&limits);
        let mut mce = Processor::new(&mut reader);
        let w = reader.intern("urn:w");
        mce.understand(w);
        let mut result = Ok(());
        while let Some(event) = reader.read_event().unwrap() {
            if let Event::Start(tag) = event {
                result = mce.start(&reader, &tag).map(|_| ());
                if result.is_ok() && tag.name().local() == "Choice" {
                    result = mce.is_selectable(&reader, &tag).map(|_| ());
                }
                if result.is_err() {
                    break;
                }
            }
        }
        assert!(
            matches!(
                result,
                Err(Error::LimitExceeded {
                    limit: Limit::MceNames,
                    maximum: 2,
                    ..
                })
            ),
            "{body}: {result:?}"
        );
    }
}

#[test]
fn checks_must_understand_only_on_elements_that_are_read() {
    // An ignored element, and an element nobody reads into, are kept as they are, so
    // what they must have understood does not matter (ECMA-376 Part 3, MustUnderstand
    // applies to the element as processed).
    assert_eq!(
        walk(
            r#"<a:x mc:Ignorable="a" mc:MustUnderstand="b"/><b:y mc:MustUnderstand="b"/>"#,
            &["urn:w"]
        )
        .unwrap()[..1],
        ["x=Ignore"]
    );
    assert!(matches!(
        walk(r#"<w:p mc:Ignorable="a" mc:MustUnderstand="b"/>"#, &["urn:w"]),
        Err(Error::Unsupported(message)) if message.starts_with("the document requires namespace urn:b")
    ));
}
