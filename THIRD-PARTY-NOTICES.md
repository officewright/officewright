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
read by `crates/officewright-font/tests/third_party_fonts.rs`, which the
`officewright-font` crate excludes from its package, and seed the fuzz targets in
`fuzz/`. Each font is unmodified and sits next to the license file it was distributed
with, byte for byte.

### Liberation Sans

- Files: `tests/fonts/liberation/LiberationSans-Regular.ttf`
  (SHA-256 `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`) and
  `tests/fonts/liberation/LICENSE`
  (SHA-256 `93fed46019c38bbe566b479d22148e2e8a1e85ada614accb0211c37b2c61c19b`), from
  `liberation-fonts-ttf-2.1.5.tar.gz` of the Liberation Fonts 2.1.5 release:
  <https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz>
  (archive SHA-256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`).
- License: SIL Open Font License 1.1. Digitized data copyright (c) 2010 Google
  Corporation with Reserved Font Arimo, Tinos and Cousine; copyright (c) 2012 Red Hat,
  Inc. with Reserved Font Name Liberation.
- Why: a hinted TrueType font with composite glyphs and Cyrillic, metric-compatible
  with Arial.

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
