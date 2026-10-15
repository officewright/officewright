//! The settings part (`w:settings`, ECMA-376 Part 1, §17.15): each setting is kept
//! whole, and those that bear on layout are typed.

use officewright_opc::mce::Processor;
use officewright_opc::xml::{Event, Input};

use super::capture::{Budget, Capture};
use super::properties::{Report, integer, on_off, value};
use super::{R, R_STRICT, W, W_STRICT, section, shorten};
use crate::diagnostic::Diagnostics;
use crate::model::properties::Language;
use crate::model::settings::{
    ColorSchemeIndex, ColorSchemeMapping, CompatibilityMode, CompatibilityOption,
    CompatibilitySetting, Settings,
};
use crate::model::xml::Element;
use crate::model::{Document, Names};
use crate::units::twips_measure;
use crate::{Error, Limits, Result};

/// The namespace of Word's own compatibility settings ([MS-DOCX] 2.3.5).
const WORD_SETTINGS: &str = "http://schemas.microsoft.com/office/word";

pub(super) fn read(
    document: &mut Document,
    xml: &[u8],
    part: &str,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    budget: &mut Budget,
) -> Result<()> {
    let opc = |err: officewright_opc::Error| Error::from(err.in_part(part));
    let input = Input::decode(xml).map_err(opc)?;
    let mut reader = input.reader(limits.package());
    let mut mce = Processor::new(&mut reader);
    let w = [reader.intern(W), reader.intern(W_STRICT)];
    for namespace in w.iter().chain(&[reader.intern(R), reader.intern(R_STRICT)]) {
        mce.understand(*namespace);
    }
    let names = document.names;
    let mut settings = Settings::default();
    let mut c = Capture::new(document, part, input.as_str(), diagnostics, budget);
    let mut in_root = false;
    while let Some(event) = reader.read_event().map_err(opc)? {
        match event {
            Event::Start(tag) => {
                c.at = tag.offset();
                let disposition = mce.start(&reader, &tag).map_err(opc)?;
                if !in_root {
                    if !w
                        .iter()
                        .any(|&namespace| tag.name().is(namespace, "settings"))
                    {
                        let message = format!(
                            "the root element is <{}>, not w:settings",
                            shorten(tag.name().qname().as_str())
                        );
                        return Err(opc(reader.error(tag.offset(), message)));
                    }
                    in_root = true;
                    continue;
                }
                let offset = tag.offset();
                // Every setting is kept with the document's tokens for writing. An
                // mc:AlternateContent is kept whole, and the children of the branch it
                // takes are read in its place; so are the children of an ignorable element
                // whose content mc:ProcessContent names.
                let span = c.capture(&mut reader, &mut mce, tag, Some(disposition))?;
                for span in c.through_alternates(span) {
                    c.check(span, offset, |element, report| {
                        if let Some(local) = element.local_in(&names.w) {
                            setting(&mut settings, element, local, &names, report);
                        }
                    });
                }
            }
            Event::End(_) => mce.end(),
            _ => {}
        }
    }
    c.document.settings = settings;
    Ok(())
}

/// Reads one child of `w:settings` into `settings`.
fn setting(
    settings: &mut Settings,
    element: Element<'_>,
    local: &str,
    names: &Names,
    report: Report<'_>,
) {
    let w = &names.w;
    let string = || element.attribute(w, "val").map(Into::into);
    match local {
        "compat" => compat(settings, element, names, report),
        "defaultTabStop" => {
            settings.default_tab_stop =
                value(element, names, "val", |v| twips_measure(v, false), report);
        }
        "evenAndOddHeaders" => settings.even_and_odd_headers = on_off(element, names, report),
        "mirrorMargins" => settings.mirror_margins = on_off(element, names, report),
        "gutterAtTop" => settings.gutter_at_top = on_off(element, names, report),
        "bordersDoNotSurroundHeader" => {
            settings.borders_do_not_surround_header = on_off(element, names, report);
        }
        "bordersDoNotSurroundFooter" => {
            settings.borders_do_not_surround_footer = on_off(element, names, report);
        }
        "autoHyphenation" => settings.auto_hyphenation = on_off(element, names, report),
        "consecutiveHyphenLimit" => {
            settings.consecutive_hyphen_limit = value(element, names, "val", integer, report);
        }
        "hyphenationZone" => {
            settings.hyphenation_zone =
                value(element, names, "val", |v| twips_measure(v, false), report);
        }
        "doNotHyphenateCaps" => settings.do_not_hyphenate_caps = on_off(element, names, report),
        "decimalSymbol" => settings.decimal_symbol = string(),
        "listSeparator" => settings.list_separator = string(),
        "defaultTableStyle" => settings.default_table_style = string(),
        "footnotePr" => {
            settings.footnotes = Some(section::notes(element, names, false, report));
            settings.footnote_separators = separators(element, names, "footnote", report);
        }
        "endnotePr" => {
            settings.endnotes = Some(section::notes(element, names, true, report));
            settings.endnote_separators = separators(element, names, "endnote", report);
        }
        "themeFontLang" => {
            settings.theme_font_languages = Some(Language {
                latin: element.attribute(w, "val").map(Into::into),
                east_asia: element.attribute(w, "eastAsia").map(Into::into),
                bidi: element.attribute(w, "bidi").map(Into::into),
            });
        }
        "clrSchemeMapping" => {
            let index = |attribute, report: Report<'_>| {
                value(
                    element,
                    names,
                    attribute,
                    ColorSchemeIndex::from_xml,
                    report,
                )
            };
            settings.color_scheme_mapping = Some(ColorSchemeMapping {
                background1: index("bg1", report),
                text1: index("t1", report),
                background2: index("bg2", report),
                text2: index("t2", report),
                accents: [
                    index("accent1", report),
                    index("accent2", report),
                    index("accent3", report),
                    index("accent4", report),
                    index("accent5", report),
                    index("accent6", report),
                ],
                hyperlink: index("hyperlink", report),
                followed_hyperlink: index("followedHyperlink", report),
            });
        }
        _ => {}
    }
}

/// `w:compat`: the legacy options and the compatibility settings.
fn compat(settings: &mut Settings, element: Element<'_>, names: &Names, report: Report<'_>) {
    let w = &names.w;
    for child in element.children_through_alternates(names.mc) {
        let Some(local) = child.local_in(w) else {
            continue;
        };
        if local != "compatSetting" {
            // An element of w:compat that is not an option of the schema is kept for
            // writing only.
            if let Some(option) = CompatibilityOption::from_xml(local) {
                if let Some(on) = on_off(child, names, report) {
                    settings.compatibility_options.push((option, on));
                }
            }
            continue;
        }
        let Some(name) = child.attribute(w, "name") else {
            continue;
        };
        let uri = child.attribute(w, "uri");
        if name == "compatibilityMode" && uri == Some(WORD_SETTINGS) {
            settings.compatibility_mode = value(
                child,
                names,
                "val",
                |v| v.trim().parse().ok().map(CompatibilityMode::new),
                report,
            );
        }
        settings.compatibility_settings.push(CompatibilitySetting {
            name: name.into(),
            uri: uri.map(Into::into),
            value: child.attribute(w, "val").map(Into::into),
        });
    }
}

/// The ids of the separator notes in a `w:footnotePr` or `w:endnotePr` of the settings.
fn separators(element: Element<'_>, names: &Names, local: &str, report: Report<'_>) -> Vec<i64> {
    element
        .children_through_alternates(names.mc)
        .filter(|child| child.is(&names.w, local))
        .filter_map(|child| value(child, names, "id", integer, report))
        .collect()
}
