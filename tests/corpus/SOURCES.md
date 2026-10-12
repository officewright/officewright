# Corpus sources

Every file under `tests/corpus/` is listed here with its origin and license. Only files
whose license allows redistribution are committed; never personal, customer or
copyrighted documents. Each file is under 1 MB and the whole corpus under 50 MB.

## Documents

| File | Origin | License | What it covers |
|------|--------|---------|----------------|

## Reference renders (`reference/`)

PDFs exported by Microsoft Word, Excel or PowerPoint from the documents above
(`scripts/windows/make-reference.ps1`). A LibreOffice export is a secondary reference
and says so in the Application column.

| File | Rendered from | Application and version | Date |
|------|---------------|-------------------------|------|

## External corpora

Not committed. `scripts/fetch-corpus.sh` downloads them into `corpus-ext/`; their
sources, pins and licenses are in `scripts/corpus-ext.tsv`.
