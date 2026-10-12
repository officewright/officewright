# Corpus sources

Every file under `tests/corpus/` is listed here with its origin and license. Only files
whose license allows redistribution are committed; never personal, customer or
copyrighted documents. Each file is under 1 MB and the whole corpus under 50 MB.

## Documents

The documents under `word-rules/` are written by `scripts/make-rule-docs.py`, one layout
rule per document (and one document per compatibility mode or `w:compat` flag when the rule
depends on it). `scripts/check.sh` fails when a committed file differs from what the script
writes.

| File | Origin | License | What it covers |
|------|--------|---------|----------------|
| word-rules/E-line-fonts.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Single line height per font: one page per font with a shaded paragraph of ten lines at 11 pt and one at 12 pt; Aptos, Cascadia Mono and Noto Sans set USE_TYPO_METRICS |
| word-rules/E-line-exact.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Exact line spacing (8, 12, 20, 30 pt) in Calibri 11: where the baseline sits in the line box and whether glyphs are clipped; one case mixes in a 28 pt run |
| word-rules/E-line-at-least.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | At-least line spacing (6, 12, 14, 20, 30 pt) in Calibri 11: when the minimum applies and where the extra space goes; one case mixes in a 28 pt run |
| word-rules/E-line-multiple.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Multiple line spacing (0.5 to 3 lines, including 1.08 and 1.15) in Calibri 11: line height and baseline position; one case mixes in a 28 pt run |
| word-rules/E-advance-rounding-12.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Glyph advance rounding: lines of 100 'i', 100 '.', 30 'm' and 50 '0' in Calibri and Times New Roman at 10, 10.5, 11 and 12 pt; no compatibilityMode setting, so Word's default, mode 12 |
| word-rules/E-advance-rounding-14.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Glyph advance rounding: lines of 100 'i', 100 '.', 30 'm' and 50 '0' in Calibri and Times New Roman at 10, 10.5, 11 and 12 pt; compatibility mode 14 |
| word-rules/E-advance-rounding-15.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Glyph advance rounding: lines of 100 'i', 100 '.', 30 'm' and 50 '0' in Calibri and Times New Roman at 10, 10.5, 11 and 12 pt; compatibility mode 15 |
| word-rules/E-space-before-top.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Space before (24 pt) of a paragraph at the top of a page: document start, after page breaks, page break before, flowed, section start, in table cells; shaded 20 pt exact lines mark the paragraph boxes |
| word-rules/E-autospacing.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Automatic paragraph spacing (beforeAutospacing, afterAutospacing) without the compat flag doNotUseHTMLParagraphAutoSpacing; shaded 20 pt exact lines mark the paragraph boxes |
| word-rules/E-autospacing-nohtml.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Automatic paragraph spacing (beforeAutospacing, afterAutospacing) with the compat flag doNotUseHTMLParagraphAutoSpacing; shaded 20 pt exact lines mark the paragraph boxes |
| word-rules/E-table-indent-14.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Horizontal position of table borders and cell text against tblInd, cell margins and alignment, in compatibility mode 14 |
| word-rules/E-table-indent-15.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Horizontal position of table borders and cell text against tblInd, cell margins and alignment, in compatibility mode 15 |
| word-rules/E-list-shared-abstract.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | List numbering across w:num instances that share one w:abstractNum, with and without startOverride, interleaved with a list of another abstract definition |
| word-rules/E-tab-hanging.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Where a tab after a label goes when the paragraph has a hanging indent, for typed labels and list labels, without the compat flag doNotUseIndentAsNumberingTabStop (default tab stops every 36 pt) |
| word-rules/E-tab-hanging-noindenttab.docx | Written by scripts/make-rule-docs.py | BUSL-1.1 (this repository) | Where a tab after a label goes when the paragraph has a hanging indent, for typed labels and list labels, with the compat flag doNotUseIndentAsNumberingTabStop (default tab stops every 36 pt) |

## Reference renders (`reference/`)

Made from PDFs exported by Microsoft Word, Excel or PowerPoint from the documents above
(`scripts/windows/make-reference.ps1`). A LibreOffice export is a secondary reference
and says so in the Application column.

The PDFs are not committed: they hold subsets of the fonts the application used,
Microsoft's among them. The maintainer keeps them; each row gives the PDF's SHA-256. For
every document `scripts/corpus-import.py` writes a folder `reference/<group>/<name>/` with
`page-001.png`, ... (every page rasterized by poppler's `pdftoppm` at 110 dpi in 8-bit
grey) and `geometry.json` (page sizes, the boxes of every line and word from
`pdftotext -bbox-layout` in points from the top left of the page, the page count, and Word's
page breaks). Poppler rasterizes fonts the PDF does not embed with substitutes, so the Notes
list them: their glyphs in the PNG files are not Word's, their positions in `geometry.json` are.

Word saves each document again before it exports it, and the PDF is made from that saved
copy, which is kept under `resaved/` with the same path: its `w:lastRenderedPageBreak`
marks are Word's page breaks for that PDF. The Notes give the compatibility mode Word used,
the page count, Word's printer (Microsoft Print to PDF, or a note that another printer was
used, without its name), the fonts Word did not have and substituted, how fields were
rendered, the PDF's SHA-256 and the poppler version.

| File | Rendered from | Application and version | Date | Notes |
|------|---------------|-------------------------|------|-------|

## External corpora

Not committed. `scripts/fetch-corpus.sh` downloads them into `corpus-ext/`; their
sources, pins and licenses are in `scripts/corpus-ext.tsv`.
