# Third-party notices

Officewright is licensed under the Business Source License 1.1 (see [LICENSE](LICENSE)).
The files listed here are not: they keep their own terms, stay in this repository and are
left out of the published crates.

## Adobe Core 14 AFM files

Copyright (c) 1985, 1987, 1989, 1990, 1997 Adobe Systems Incorporated. All Rights
Reserved.

- Files: `crates/officewright-pdf/data/afm/Helvetica.afm` and
  `crates/officewright-pdf/data/afm/MustRead.html`, byte for byte as in Adobe's
  `Core14_AFMs.tar`, formerly published at
  `http://partners.adobe.com/public/developer/en/pdf/Core14_AFMs.tar`.
- Retrieved from the Internet Archive:
  <https://web.archive.org/web/20120324112255id_/http://partners.adobe.com/public/developer/en/pdf/Core14_AFMs.tar>
  (archive SHA-256 `fb0736bab04daf130998f5e5b09c61307fd0e48c778bff1a24bf23591a56467a`).
- Derived work: the table of Helvetica advance widths in
  `crates/officewright-pdf/src/helvetica.rs` is derived from `Helvetica.afm` (the
  widths of the 218 glyphs of WinAnsiEncoding, indexed by code). That table is compiled
  into the `officewright-pdf` crate and the `officewright` native library; the AFM
  files are not. `crates/officewright-pdf/tests/helvetica_afm.rs` checks the table
  against the AFM file.

Adobe's terms, from `MustRead.html`:

> This file and the 14 PostScript(R) AFM files it accompanies may be used, copied, and distributed for any purpose and without charge, with or without modification, provided that all copyright notices are retained; that the AFM files are not distributed without this file; that all modifications to this file or any of the AFM files are prominently noted in the modified file(s); and that this paragraph is not modified. Adobe Systems has no responsibility or obligation to support the use of the AFM files.

## Documents of the European Union

© European Union. Licensed under the Creative Commons Attribution 4.0
International licence (CC BY 4.0, <https://creativecommons.org/licenses/by/4.0/>), as
the European Commission's legal notice (<https://commission.europa.eu/legal-notice_en>)
states for the content it owns: "Unless otherwise indicated (e.g. in individual copyright
notices), content owned by the EU on this website is licensed under the Creative Commons
Attribution 4.0 International (CC BY 4.0) licence. This means that reuse is allowed,
provided appropriate credit is given and changes are indicated."

Test documents in `tests/corpus/business/`, outside every crate and never published:

- `form-ec-state-aid-notification-de.docx`, `form-ec-state-aid-notification-bg.docx` and
  `form-ec-state-aid-notification-el.docx`: the Part I general information form for
  notifying State aid, in German, Bulgarian and Greek. Source: European Commission,
  Directorate-General for Competition,
  <https://competition-policy.ec.europa.eu/state-aid/legislation/forms-notifications-and-reporting_en>
  (archive `forms_notifications_and_reporting_part_I.zip`).
- `contract-ec-data-processing-clauses-de.docx`,
  `contract-ec-data-processing-clauses-bg.docx` and
  `contract-ec-data-processing-clauses-el.docx`: the annex with the standard contractual
  clauses between controllers and processors (Commission Implementing Decision (EU)
  2021/915), in German, Bulgarian and Greek. Source: European Commission,
  <https://commission.europa.eu/publications/standard-contractual-clauses-controllers-and-processors-eueea_en>
  (archive `strandard_contractual_clauses_eu-eea.zip`).

Changes: the document properties' custom fields, the custom XML parts and the attached
template are removed, and every picture (the EU emblem among them, which the legal notice
excludes from the licence) is replaced by a grey placeholder of the same size.
`tests/corpus/SOURCES.md` gives each file's URL, download date, original SHA-256 and its
changes; `scripts/import-public-docs.py` makes them.

Derived from these documents, under the same licence and credit: the copies Microsoft Word
saved of the three State aid forms (`tests/corpus/resaved/business/form-ec-state-aid-notification-de.docx`,
`-bg.docx` and `-el.docx`) and their reference renders
(`tests/corpus/reference/business/form-ec-state-aid-notification-de/`, `-bg/` and `-el/`:
every page as a grey image, the position of every line and word, and the formatting Word
computes). Changes: Word saved each document again, and its PDF of it was rasterized and
measured by `scripts/corpus-import.py`.

## Test fonts

Fonts in `tests/fonts/`, at the repository root and so outside every crate. They are
read by `crates/officewright-font/tests/third_party_fonts.rs`,
`crates/officewright-font/tests/metric_compatible.rs`,
`crates/officewright-pdf/tests/embedded_fonts.rs`,
`crates/officewright-layout/tests/shaping.rs` and
`crates/officewright-cli/tests/fonts.rs`, which their crates exclude from their
packages, by the C ABI tests in `tests/c` and `tests/ffi-python`, the PDF output
checks in `tests/pdf-output` and `tests/corpus-tools/poppler/test_font_embedding.py`,
and `tests/corpus-tools/test_test_fonts.py` checks this list of them; they seed the fuzz
targets in `fuzz/`. The tests embed
them, reduced to some of their glyphs, in PDF files they write to temporary directories;
those files are not committed or published. Each font is unmodified and sits next to the
license file it was distributed with, byte for byte.

### Liberation Sans, Serif and Mono

- Files:
  `tests/fonts/liberation/LiberationSans-Regular.ttf`
  (SHA-256 `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`),
  `tests/fonts/liberation/LiberationSans-Bold.ttf`
  (SHA-256 `788abee4c806d660e8aee46689dd8540cd4bb98da03dcc9d171ce3efd99a9173`),
  `tests/fonts/liberation/LiberationSans-Italic.ttf`
  (SHA-256 `e5bae5c4cde31f22142753855f4f8fb86da6ff39955ed3c0a11248b0d16948b0`),
  `tests/fonts/liberation/LiberationSans-BoldItalic.ttf`
  (SHA-256 `698da70fc191cc5f33ad4d6d3fe830fe4624b898ea2e3169955928b7c491f1ee`),
  `tests/fonts/liberation/LiberationSerif-Regular.ttf`
  (SHA-256 `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74`),
  `tests/fonts/liberation/LiberationSerif-Bold.ttf`
  (SHA-256 `d754ba427cfe0bca54ae052384baa8f842da5bd6550ad4da024ac441e7a7d5ce`),
  `tests/fonts/liberation/LiberationSerif-Italic.ttf`
  (SHA-256 `0e3dea9f8d613e006ccfa62201f33e265d19167bd0907725c3e145368b04fc2e`),
  `tests/fonts/liberation/LiberationSerif-BoldItalic.ttf`
  (SHA-256 `f17db8af71e24d2066b587546021d4f0b296be389512b658dec3c09affeb11a7`),
  `tests/fonts/liberation/LiberationMono-Regular.ttf`
  (SHA-256 `f2b83c763e8afd21709333370bed4774337fae82267937e2b5aea7e2fbd922c1`),
  `tests/fonts/liberation/LiberationMono-Bold.ttf`
  (SHA-256 `bd62a0672d0b9b6710b01df434c80ad54fa5f0835207eb7b17b7a761463067bb`),
  `tests/fonts/liberation/LiberationMono-Italic.ttf`
  (SHA-256 `605c01c711b44480a7508d349dfbf3264e81fa43d69e61cfa7d10b86e764c4d1`),
  `tests/fonts/liberation/LiberationMono-BoldItalic.ttf`
  (SHA-256 `79451f3c09fe25116098853b7a2ca6e2436220ccc11af022979adbcf195be130`) and
  `tests/fonts/liberation/LICENSE`
  (SHA-256 `93fed46019c38bbe566b479d22148e2e8a1e85ada614accb0211c37b2c61c19b`), from
  `liberation-fonts-ttf-2.1.5.tar.gz` of the Liberation Fonts 2.1.5 release:
  <https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz>
  (archive SHA-256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`).
- License: SIL Open Font License 1.1. Digitized data copyright (c) 2010 Google
  Corporation with Reserved Font Arimo, Tinos and Cousine; copyright (c) 2012 Red Hat,
  Inc. with Reserved Font Name Liberation.
- Why: hinted TrueType fonts with composite glyphs and Cyrillic, metric-compatible
  with Arial, Times New Roman and Courier New; `metric_compatible.rs` measures them
  against Word's rendering of those fonts. The bold, italic and bold italic styles are
  there because documents of the corpus use them: Word lays those out with the installed
  families, and the corpus runner's layout should use the same faces, not synthesized
  ones.

### Carlito

- Files:
  `tests/fonts/carlito/Carlito-Regular.ttf`
  (SHA-256 `f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595`),
  `tests/fonts/carlito/Carlito-Bold.ttf`
  (SHA-256 `bb5d20f79b82599ec72983597437373a80f2d2085fa91fc144fd74e876a594db`),
  `tests/fonts/carlito/Carlito-Italic.ttf`
  (SHA-256 `0b019225e58d702bfedcbd35c21696769f8ee115cb6343f84c2f240312450d1c`),
  `tests/fonts/carlito/Carlito-BoldItalic.ttf`
  (SHA-256 `b32928186c119599e03ca6a1ffc680fdcb7fac95772f4b95d989cf6cd3861517`) and
  `tests/fonts/carlito/OFL.txt`
  (SHA-256 `58402f82a7c332a700294988fe7554fbb0a63a8d27ccc1ee3bbc640311990a00`), the files
  `fonts/ttf/Carlito-Regular.ttf`, `Carlito-Bold.ttf`, `Carlito-Italic.ttf`,
  `Carlito-BoldItalic.ttf` and `OFL.txt` of <https://github.com/googlefonts/carlito> at
  commit `3a810cab78ebd6e2e4eed42af9e8453c4f9b850a` (Carlito 1.104).
- License: SIL Open Font License 1.1. Copyright 2013 The Carlito Project Authors, with
  Reserved Font Name "Carlito".
- Why: metric-compatible with Calibri; `metric_compatible.rs` measures it against
  Word's rendering of Calibri. Its bold, italic and bold italic styles are there for the
  documents of the corpus that use them, as for Liberation.

### Noto Sans

- Files: `tests/fonts/noto/NotoSans-Regular.otf`
  (SHA-256 `bfb44de2c2031c391ff856943fe8f105ebb5d725cb9a04d493d211d5bbd5c21b`) and
  `tests/fonts/noto/LICENSE`
  (SHA-256 `f2095b08bed08b23a6fe26112fcd679a2bee3f002eef077eb05d215ed1051bd8`), the files
  `fonts/NotoSans/unhinted/otf/NotoSans-Regular.otf` and `fonts/LICENSE` of
  <https://github.com/notofonts/notofonts.github.io> at commit
  `28b15b4b43b7bed62b5cf6e6b0b5ff5846270535`.
- License: SIL Open Font License 1.1. Copyright 2022 The Noto Project Authors.
- Why: an OpenType font with CFF outlines.

## Apache POI test documents

Copyright 2003-2026 The Apache Software Foundation. Licensed under the Apache License,
Version 2.0.

- Files: the 16 Word documents in `tests/corpus/poi/`, each listed with its SHA-256 in
  `tests/corpus/SOURCES.md`, byte for byte as in Apache POI's `test-data/document` at
  commit `f3688f45b0de8934fb32736ba14efc5bbca4a580`
  (<https://github.com/apache/poi/tree/f3688f45b0de8934fb32736ba14efc5bbca4a580/test-data/document>).
- Next to them, unmodified from POI's `legal/` at that commit: `tests/corpus/poi/LICENSE`
  (SHA-256 `6e7c918e5a49f677c1b85a9eb2d035cd85c217fc42af6b4a336964a434001a9c`), the
  Apache License 2.0 with the notes on the parts of POI under other licenses, and
  `tests/corpus/poi/NOTICE`
  (SHA-256 `a89ff8b670bcacd3e98ebee64979e0569181c7942e9ad795f4222b25e5637a61`).
- They are test input of `tools/corpus-runner` (run by `scripts/check.sh`) and seed the
  `docx_open` fuzz target; they sit outside every crate and are never published.
