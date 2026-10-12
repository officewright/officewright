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

## Test fonts

Fonts in `tests/fonts/`, at the repository root and so outside every crate. They are
read by `crates/officewright-font/tests/third_party_fonts.rs`,
`crates/officewright-font/tests/metric_compatible.rs`,
`crates/officewright-pdf/tests/embedded_fonts.rs` and
`crates/officewright-cli/tests/fonts.rs`, which their crates exclude from their
packages, by the C ABI tests in `tests/c` and `tests/ffi-python` and the PDF output
checks in `tests/pdf-output`, and they seed the fuzz targets in `fuzz/`. The tests embed
them, reduced to some of their glyphs, in PDF files they write to temporary directories;
those files are not committed or published. Each font is unmodified and sits next to the
license file it was distributed with, byte for byte.

### Liberation Sans, Serif and Mono

- Files: `tests/fonts/liberation/LiberationSans-Regular.ttf`
  (SHA-256 `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`),
  `tests/fonts/liberation/LiberationSerif-Regular.ttf`
  (SHA-256 `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74`),
  `tests/fonts/liberation/LiberationMono-Regular.ttf`
  (SHA-256 `f2b83c763e8afd21709333370bed4774337fae82267937e2b5aea7e2fbd922c1`) and
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
  against Word's rendering of those fonts.

### Carlito

- Files: `tests/fonts/carlito/Carlito-Regular.ttf`
  (SHA-256 `f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595`) and
  `tests/fonts/carlito/OFL.txt`
  (SHA-256 `58402f82a7c332a700294988fe7554fbb0a63a8d27ccc1ee3bbc640311990a00`), the files
  `fonts/ttf/Carlito-Regular.ttf` and `OFL.txt` of
  <https://github.com/googlefonts/carlito> at commit
  `3a810cab78ebd6e2e4eed42af9e8453c4f9b850a` (Carlito 1.104).
- License: SIL Open Font License 1.1. Copyright 2013 The Carlito Project Authors, with
  Reserved Font Name "Carlito".
- Why: metric-compatible with Calibri; `metric_compatible.rs` measures it against
  Word's rendering of Calibri.

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
