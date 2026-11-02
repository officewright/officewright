/* Calls every function of the C ABI through the static library and checks statuses,
 * error messages and out-parameters. Built with AddressSanitizer it also proves that
 * handles and buffers are released exactly once. Compiled as C and as C++ so the
 * header is proven usable from both.
 *
 * Usage: abi_test <directory for output files> <the repository's tests/fonts> */
#define _CRT_SECURE_NO_WARNINGS /* MSVC: plain fopen is what this test needs */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "officewright.h"

static int failures = 0;

static void check(int ok, const char *what, int line) {
    if (!ok) {
        fprintf(stderr, "abi_test.c:%d: check failed: %s\n", line, what);
        failures++;
    }
}
#define CHECK(cond) check((cond) != 0, #cond, __LINE__)

/* Checks the status of a call and that the last error contains `part`, or is empty
 * when `part` is NULL. */
static void expect(OwStatus got, OwStatus want, const char *part, const char *call, int line) {
    const char *message = ow_last_error_message();
    if (got != want) {
        fprintf(stderr, "abi_test.c:%d: %s returned %d, expected %d (last error: %s)\n", line,
                call, (int)got, (int)want, message);
        failures++;
    } else if (part == NULL ? message[0] != '\0' : strstr(message, part) == NULL) {
        fprintf(stderr, "abi_test.c:%d: %s: last error is \"%s\", expected \"%s\"\n", line,
                call, message, part == NULL ? "" : part);
        failures++;
    }
}
#define EXPECT(call, want, part) expect((call), (want), (part), #call, __LINE__)

static int contains(const uint8_t *data, size_t len, const char *needle) {
    size_t n = strlen(needle);
    size_t i;
    for (i = 0; i + n <= len; i++) {
        if (memcmp(data + i, needle, n) == 0) {
            return 1;
        }
    }
    return 0;
}

static int file_equals(const char *path, const uint8_t *data, size_t len) {
    FILE *file = fopen(path, "rb");
    size_t i;
    int same = 1;
    if (file == NULL) {
        return 0;
    }
    for (i = 0; i < len && same; i++) {
        int c = fgetc(file);
        same = c != EOF && (uint8_t)c == data[i];
    }
    if (same && fgetc(file) != EOF) {
        same = 0;
    }
    fclose(file);
    return same;
}

static OwPdfFont *helvetica(void) {
    OwPdfFont *font = NULL;
    EXPECT(ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, &font), OW_STATUS_OK, NULL);
    return font;
}

static void build_and_save(const char *dir) {
    OwPdfDocument *doc = NULL;
    OwPdfFont *font = helvetica();
    size_t page = 99;
    uint8_t *data = NULL;
    size_t len = 0;
    char path[4096];

    EXPECT(ow_pdf_document_new(&doc), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_page(doc, 595.0, 842.0, &page), OW_STATUS_OK, NULL);
    CHECK(page == 0);
    /* "Gruesse (5 euro)" with u-umlaut, sharp s and the euro sign, in UTF-8; all of it
     * is in WinAnsiEncoding. */
    EXPECT(ow_pdf_document_add_text(doc, page, 72.0, 770.0, font, 12.0,
                                    "Gr\xC3\xBC\xC3\x9F" "e (\xE2\x82\xAC" "5)"),
           OW_STATUS_OK, NULL);
    /* The document keeps what it needs, so the font can go before the document. */
    ow_pdf_font_free(font);
    /* Uncompressed, to read the page content. */
    EXPECT(ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE), OW_STATUS_OK, NULL);

    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_OK, NULL);
    CHECK(data != NULL && len > 20);
    CHECK(memcmp(data, "%PDF-1.4\n", 9) == 0);
    CHECK(memcmp(data + len - 6, "%%EOF\n", 6) == 0);
    CHECK(contains(data, len, "(Gr\\374\\337e \\(\\2005\\)) Tj"));

    snprintf(path, sizeof path, "%s/hello.pdf", dir);
    EXPECT(ow_pdf_document_save_path(doc, path), OW_STATUS_OK, NULL);
    CHECK(file_equals(path, data, len));

    ow_bytes_free(data, len);
    ow_pdf_document_free(doc);
}

static void text_layout_and_width(void) {
    OwPdfFont *font = helvetica();
    OwPdfDocument *doc = NULL;
    uint8_t *data = NULL;
    size_t len = 0;
    double width = 0.0;

    EXPECT(ow_pdf_document_from_text("Hello,\nworld\fpage two", 612.0, 792.0, font, 11.0, &doc),
           OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_OK, NULL);
    CHECK(contains(data, len, "/Count 2"));
    CHECK(contains(data, len, "(page two) Tj"));
    ow_bytes_free(data, len);
    ow_pdf_document_free(doc);

    /* Helvetica's "H" is 722/1000 em wide. */
    EXPECT(ow_pdf_font_text_width(font, "H", 10.0, &width), OW_STATUS_OK, NULL);
    CHECK(width > 7.2199 && width < 7.2201);
    ow_pdf_font_free(font);
}

static void compression(void) {
    OwPdfFont *font = helvetica();
    OwPdfDocument *doc = NULL;
    uint8_t *compressed = NULL;
    uint8_t *plain = NULL;
    size_t compressed_len = 0;
    size_t plain_len = 0;
    char text[13 * 40 + 1] = "";
    int i;

    for (i = 0; i < 40; i++) {
        strcat(text, "Hello, world\n");
    }
    EXPECT(ow_pdf_document_from_text(text, 595.0, 842.0, font, 11.0, &doc), OW_STATUS_OK, NULL);
    ow_pdf_font_free(font);
    /* Compressed unless set otherwise. */
    EXPECT(ow_pdf_document_save_bytes(doc, &compressed, &compressed_len), OW_STATUS_OK, NULL);
    CHECK(contains(compressed, compressed_len, "/Filter /FlateDecode"));
    CHECK(!contains(compressed, compressed_len, "(Hello, world) Tj"));

    EXPECT(ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(doc, &plain, &plain_len), OW_STATUS_OK, NULL);
    CHECK(!contains(plain, plain_len, "/Filter"));
    CHECK(contains(plain, plain_len, "(Hello, world) Tj"));
    CHECK(plain_len > compressed_len);
    ow_bytes_free(plain, plain_len);

    EXPECT(ow_pdf_document_set_compression(doc, 2), OW_STATUS_INVALID_ARGUMENT,
           "compression is 2, not OW_COMPRESSION_NONE (0) or OW_COMPRESSION_FLATE (1)");
    EXPECT(ow_pdf_document_set_compression(doc, OW_COMPRESSION_FLATE), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(doc, &plain, &plain_len), OW_STATUS_OK, NULL);
    CHECK(plain_len == compressed_len && memcmp(plain, compressed, plain_len) == 0);
    ow_bytes_free(plain, plain_len);
    ow_bytes_free(compressed, compressed_len);

    EXPECT(ow_pdf_document_set_compression(NULL, OW_COMPRESSION_NONE), OW_STATUS_NULL_ARGUMENT,
           "doc is null");
    ow_pdf_document_free(doc);
}

static void errors(void) {
    /* Statuses are only ever appended, so existing values keep their numbers. */
    CHECK(OW_STATUS_PANIC == 6 && OW_STATUS_MALFORMED == 7 && OW_STATUS_LIMIT_EXCEEDED == 8);
    /* Failing calls must reset their out-parameters, so these start as non-NULL
     * placeholders that are never dereferenced. */
    uint8_t dummy = 0;
    OwPdfFont *font = helvetica();
    OwPdfFont *unknown = (OwPdfFont *)(void *)&dummy;
    OwPdfDocument *not_made = (OwPdfDocument *)(void *)&dummy;
    OwPdfDocument *doc = NULL;
    OwPdfDocument *empty = NULL;
    size_t page = 99;
    uint8_t *data = &dummy;
    size_t len = 7;
    double width = -1.0;

    EXPECT(ow_pdf_document_new(NULL), OW_STATUS_NULL_ARGUMENT, "out_doc is null");
    EXPECT(ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, NULL), OW_STATUS_NULL_ARGUMENT,
           "out_font is null");
    EXPECT(ow_pdf_font_new_standard(7, &unknown), OW_STATUS_INVALID_ARGUMENT,
           "standard font 7 is not one of the OW_STANDARD_FONT_* values");
    CHECK(unknown == NULL);
    EXPECT(ow_pdf_document_new(&doc), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_page(NULL, 595.0, 842.0, &page), OW_STATUS_NULL_ARGUMENT,
           "doc is null");
    EXPECT(ow_pdf_document_add_page(doc, 595.0, 842.0, NULL), OW_STATUS_NULL_ARGUMENT,
           "out_page_index is null");
    page = 99;
    EXPECT(ow_pdf_document_add_page(doc, 1.0, 842.0, &page), OW_STATUS_INVALID_ARGUMENT,
           "outside the 3 to 14400 pt range");
    CHECK(page == 0);
    EXPECT(ow_pdf_document_from_text("x", 612.0, 792.0, NULL, 11.0, &not_made),
           OW_STATUS_NULL_ARGUMENT, "font is null");
    CHECK(not_made == NULL);
    EXPECT(ow_pdf_document_add_page(doc, 595.0, 842.0, &page), OW_STATUS_OK, NULL);

    EXPECT(ow_pdf_document_add_text(doc, 1, 0.0, 0.0, font, 10.0, "x"), OW_STATUS_INVALID_ARGUMENT,
           "page index 1 is out of range");
    EXPECT(ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 0.0, "x"), OW_STATUS_INVALID_ARGUMENT,
           "font size 0 pt");
    EXPECT(ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, NULL), OW_STATUS_NULL_ARGUMENT,
           "text is null");
    EXPECT(ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, "ab\xFF"), OW_STATUS_INVALID_UTF8,
           "text is not valid UTF-8 at byte 2");
    /* Cyrillic "Zhe" (U+0416) has no WinAnsiEncoding code. */
    EXPECT(ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, "\xD0\x96"), OW_STATUS_UNSUPPORTED,
           "character U+0416");

    EXPECT(ow_pdf_document_add_text(doc, 0, 0.0, 0.0, NULL, 10.0, "x"), OW_STATUS_NULL_ARGUMENT,
           "font is null");
    EXPECT(ow_pdf_document_save_path(doc, NULL), OW_STATUS_NULL_ARGUMENT, "path is null");
    EXPECT(ow_pdf_document_save_path(doc, "no-such-directory/x/out.pdf"), OW_STATUS_IO,
           "cannot write no-such-directory/x/out.pdf: ");
    EXPECT(ow_pdf_font_text_width(font, "\xD0\x96", 10.0, &width), OW_STATUS_UNSUPPORTED, "U+0416");
    CHECK(width == 0.0);

    EXPECT(ow_pdf_document_new(&empty), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(empty, &data, &len), OW_STATUS_INVALID_ARGUMENT,
           "document has no pages");
    CHECK(data == NULL && len == 0);

    ow_pdf_document_free(empty);
    ow_pdf_document_free(doc);
    ow_pdf_font_free(font);
    ow_pdf_document_free(NULL);
    ow_pdf_font_free(NULL);
    ow_bytes_free(NULL, 0);
}

/* Reads a whole file into a buffer the caller frees; NULL if it cannot. */
static uint8_t *read_file(const char *path, size_t *len) {
    FILE *file = fopen(path, "rb");
    uint8_t *data = NULL;
    long size;
    if (file == NULL) {
        return NULL;
    }
    if (fseek(file, 0, SEEK_END) == 0 && (size = ftell(file)) > 0 && fseek(file, 0, SEEK_SET) == 0) {
        data = (uint8_t *)malloc((size_t)size);
        if (data != NULL && fread(data, 1, (size_t)size, file) != (size_t)size) {
            free(data);
            data = NULL;
        }
        *len = (size_t)size;
    }
    fclose(file);
    return data;
}

/* Sets fsType in the OS/2 table of a single font file, which declares what the font's
 * license allows; returns 0 if the table is not there. */
static int set_fs_type(uint8_t *font, size_t len, unsigned fs_type) {
    size_t tables;
    size_t i;
    if (len < 12) {
        return 0;
    }
    tables = ((size_t)font[4] << 8) | font[5];
    for (i = 0; i < tables && 12 + 16 * i + 16 <= len; i++) {
        const uint8_t *record = font + 12 + 16 * i;
        if (memcmp(record, "OS/2", 4) == 0) {
            size_t os2 = ((size_t)record[8] << 24) | ((size_t)record[9] << 16) |
                         ((size_t)record[10] << 8) | record[11];
            if (os2 + 10 > len) {
                return 0;
            }
            font[os2 + 8] = (uint8_t)(fs_type >> 8);
            font[os2 + 9] = (uint8_t)(fs_type & 0xFF);
            return 1;
        }
    }
    return 0;
}

static void embedded_fonts(const char *fonts) {
    char path[4096];
    OwPdfFont *liberation = NULL;
    OwPdfFont *noto = NULL;
    OwPdfFont *whole = NULL;
    OwPdfFont *not_made = NULL;
    OwPdfDocument *doc = NULL;
    uint8_t *font_data = NULL;
    uint8_t *data = NULL;
    size_t font_len = 0;
    size_t len = 0;
    size_t count = 99;
    size_t page = 99;
    int32_t kind = -1;
    const char *message = "placeholder";
    double width = 0.0;

    /* A TrueType font from its file. */
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Regular.ttf", fonts);
    EXPECT(ow_pdf_font_open_path(path, 0, &liberation), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_font_note_count(liberation, &count), OW_STATUS_OK, NULL);
    CHECK(count == 0);
    /* A CFF font from memory; the library copies the bytes, so the buffer can go. */
    snprintf(path, sizeof path, "%s/noto/NotoSans-Regular.otf", fonts);
    font_data = read_file(path, &font_len);
    CHECK(font_data != NULL);
    EXPECT(ow_pdf_font_open_bytes(font_data, font_len, 0, &noto), OW_STATUS_OK, NULL);
    free(font_data);

    /* "Privet, mir" in Cyrillic, laid out in Liberation Sans, and a Greek line in Noto
     * Sans on the same page. */
    EXPECT(ow_pdf_document_from_text("\xD0\x9F\xD1\x80\xD0\xB8\xD0\xB2\xD0\xB5\xD1\x82, "
                                     "\xD0\xBC\xD0\xB8\xD1\x80",
                                     595.0, 842.0, liberation, 12.0, &doc),
           OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_text(doc, 0, 72.0, 600.0, noto, 12.0,
                                    "\xCE\x9A\xCE\xB1\xCE\xBB\xCE\xB7\xCE\xBC\xCE\xAD\xCF\x81\xCE\xB1"),
           OW_STATUS_OK, NULL);
    /* The document keeps what it needs, so the fonts can go before it. */
    EXPECT(ow_pdf_font_text_width(liberation, "\xD0\x96", 10.0, &width), OW_STATUS_OK, NULL);
    CHECK(width > 5.0 && width < 15.0);
    ow_pdf_font_free(noto);
    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_OK, NULL);
    CHECK(contains(data, len, "/Subtype /Type0"));
    CHECK(contains(data, len, "/FontFile2"));
    CHECK(contains(data, len, "/Subtype /CIDFontType0C"));
    CHECK(contains(data, len, "/ToUnicode"));
    ow_bytes_free(data, len);
    /* Reducing Noto Sans (CFF) to its glyphs reads its charstrings, which a budget of 0
     * does not allow; a large one does. */
    EXPECT(ow_pdf_document_set_subset_budget(doc, 0), OW_STATUS_OK, NULL);
    data = (uint8_t *)&len;
    len = 99;
    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_LIMIT_EXCEEDED,
           "hint mask bytes left of its budget");
    CHECK(data == NULL && len == 0);
    EXPECT(ow_pdf_document_set_subset_budget(doc, UINT64_MAX), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_OK, NULL);
    CHECK(contains(data, len, "/Subtype /CIDFontType0C"));
    ow_bytes_free(data, len);
    EXPECT(ow_pdf_document_set_subset_budget(NULL, 0), OW_STATUS_NULL_ARGUMENT, "doc is null");
    ow_pdf_document_free(doc);

    /* Text an embedded font cannot show: Hebrew needs right-to-left layout, and Liberation
     * Sans has no glyph for the Han character. */
    EXPECT(ow_pdf_document_new(&doc), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_page(doc, 595.0, 842.0, &page), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_text(doc, page, 0.0, 0.0, liberation, 10.0, "a\xD7\xA9"),
           OW_STATUS_UNSUPPORTED, "is in the Hebrew script");
    EXPECT(ow_pdf_document_add_text(doc, page, 0.0, 0.0, liberation, 10.0, "\xE6\xBC\xA2"),
           OW_STATUS_UNSUPPORTED, "font LiberationSans has no glyph for character U+6F22");
    ow_pdf_document_free(doc);

    /* A license that forbids subsetting and allows only previewing and printing: the
     * font is embedded whole, and both restrictions are noted. */
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Regular.ttf", fonts);
    font_data = read_file(path, &font_len);
    CHECK(font_data != NULL && set_fs_type(font_data, font_len, 0x0104));
    EXPECT(ow_pdf_font_open_bytes(font_data, font_len, 0, &whole), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_font_note_count(whole, &count), OW_STATUS_OK, NULL);
    CHECK(count == 2);
    EXPECT(ow_pdf_font_note(whole, 0, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_FONT_NOTE_PREVIEW_AND_PRINT);
    EXPECT(ow_pdf_font_note(whole, 1, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_FONT_NOTE_WHOLE_FONT);
    CHECK(message != NULL && strstr(message, "forbids subsetting") != NULL);
    EXPECT(ow_pdf_font_note(whole, 2, &kind, &message), OW_STATUS_INVALID_ARGUMENT,
           "note index 2 is out of range for a font with 2 notes");
    CHECK(kind == OW_FONT_NOTE_OTHER && message == NULL);
    ow_pdf_font_free(whole);
    /* A license that forbids embedding. */
    CHECK(set_fs_type(font_data, font_len, 0x0002));
    EXPECT(ow_pdf_font_open_bytes(font_data, font_len, 0, &not_made), OW_STATUS_UNSUPPORTED,
           "does not allow embedding it");
    CHECK(not_made == NULL);
    free(font_data);

    /* Fonts that cannot be opened. */
    not_made = liberation;
    EXPECT(ow_pdf_font_open_bytes((const uint8_t *)"not a font", 10, 0, &not_made),
           OW_STATUS_MALFORMED, "cannot use the font data: data is not a TrueType");
    CHECK(not_made == NULL);
    /* A face the file does not have is the caller's mistake, not damage. */
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Regular.ttf", fonts);
    font_data = read_file(path, &font_len);
    CHECK(font_data != NULL);
    not_made = liberation;
    EXPECT(ow_pdf_font_open_bytes(font_data, font_len, 1, &not_made), OW_STATUS_INVALID_ARGUMENT,
           "font face 1 does not exist, the file has 1 face");
    CHECK(not_made == NULL);
    free(font_data);
    EXPECT(ow_pdf_font_open_bytes(NULL, 0, 0, &not_made), OW_STATUS_NULL_ARGUMENT, "data is null");
    snprintf(path, sizeof path, "%s/no-such-font.ttf", fonts);
    EXPECT(ow_pdf_font_open_path(path, 0, &not_made), OW_STATUS_IO, "cannot read ");
    EXPECT(ow_pdf_font_open_path(NULL, 0, &not_made), OW_STATUS_NULL_ARGUMENT, "path is null");
    EXPECT(ow_pdf_font_open_path(path, 0, NULL), OW_STATUS_NULL_ARGUMENT, "out_font is null");
    EXPECT(ow_pdf_font_note_count(NULL, &count), OW_STATUS_NULL_ARGUMENT, "font is null");
    CHECK(count == 0);
    ow_pdf_font_free(liberation);
}

/* Chooses fonts by family name from a book of the test fonts, without the system
 * fonts, so the result is the same on every machine. */
static void fonts_by_name(const char *fonts, const char *dir) {
    char path[4096];
    OwFontBook *book = NULL;
    OwFontBook *empty = NULL;
    OwFontResolver *resolver = NULL;
    OwFontResolver *nothing = NULL;
    OwPdfFont *liberation = NULL;
    OwPdfFont *arial = NULL;
    OwPdfFont *bold = NULL;
    OwPdfFont *not_found = NULL;
    size_t added = 99;
    size_t count = 99;
    int32_t kind = -1;
    const char *message = NULL;
    double width = 0.0;
    uint8_t *data = NULL;
    size_t len = 0;
    OwPdfDocument *doc = NULL;
    size_t page = 99;
    FILE *file;

    EXPECT(ow_font_book_new(&book), OW_STATUS_OK, NULL);
    EXPECT(ow_font_book_add_dir(book, fonts, &added), OW_STATUS_OK, NULL);
    /* Carlito and Liberation Sans, Serif and Mono in four styles each, and Noto Sans. */
    CHECK(added == 17);
    EXPECT(ow_font_book_face_count(book, &count), OW_STATUS_OK, NULL);
    CHECK(count == 17);
    EXPECT(ow_font_resolver_new(book, &resolver), OW_STATUS_OK, NULL);
    /* The resolver keeps its own copy of the book. */
    ow_font_book_free(book);

    EXPECT(ow_pdf_font_resolve(resolver, "Liberation Sans", 0, 0, &liberation), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_substitution_count(resolver, &count), OW_STATUS_OK, NULL);
    CHECK(count == 0);
    /* Arial's metric-compatible substitute, and a bold Noto Sans has no face for. */
    EXPECT(ow_pdf_font_resolve(resolver, "Arial", 0, 0, &arial), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_font_resolve(resolver, "Noto Sans", 1, 0, &bold), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_substitution_count(resolver, &count), OW_STATUS_OK, NULL);
    CHECK(count == 2);
    EXPECT(ow_font_resolver_substitution(resolver, 0, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_RESOLUTION_METRIC_COMPATIBLE);
    CHECK(message != NULL && strstr(message, "\"Arial\"") != NULL &&
          strstr(message, "\"Liberation Sans\" (metric-compatible substitute)") != NULL);
    EXPECT(ow_font_resolver_substitution(resolver, 1, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_RESOLUTION_EXACT);
    CHECK(message != NULL && strstr(message, "synthetic bold") != NULL);
    EXPECT(ow_font_resolver_substitution(resolver, 2, &kind, &message), OW_STATUS_INVALID_ARGUMENT,
           "substitution index 2 is out of range");
    CHECK(kind == OW_RESOLUTION_OTHER && message == NULL);
    /* The chosen fonts measure alike: Arial's substitute is Liberation Sans. */
    EXPECT(ow_pdf_font_text_width(arial, "Arial", 10.0, &width), OW_STATUS_OK, NULL);
    CHECK(width > 20.0 && width < 30.0);
    EXPECT(ow_pdf_font_resolve(resolver, "Liberation Sans", 2, 0, &not_found),
           OW_STATUS_INVALID_ARGUMENT, "bold is 2, not 0 or 1");
    CHECK(not_found == NULL);
    ow_font_resolver_free(resolver);
    ow_pdf_font_free(liberation);
    ow_pdf_font_free(arial);
    ow_pdf_font_free(bold);

    /* Liberation Sans without its Italic face: italic text slants Regular, as Word
     * does, rather than take Bold Italic. */
    EXPECT(ow_font_book_new(&book), OW_STATUS_OK, NULL);
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Regular.ttf", fonts);
    EXPECT(ow_font_book_add_file(book, path, &added), OW_STATUS_OK, NULL);
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-BoldItalic.ttf", fonts);
    EXPECT(ow_font_book_add_file(book, path, &added), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_new(book, &resolver), OW_STATUS_OK, NULL);
    ow_font_book_free(book);
    EXPECT(ow_pdf_font_resolve(resolver, "Liberation Sans", 0, 1, &liberation), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_substitution_count(resolver, &count), OW_STATUS_OK, NULL);
    CHECK(count == 1);
    EXPECT(ow_font_resolver_substitution(resolver, 0, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_RESOLUTION_EXACT);
    CHECK(message != NULL && strstr(message, "with synthetic italic") != NULL);
    /* Text in it is slanted by a third about its baseline, inside q ... Q. */
    EXPECT(ow_pdf_document_new(&doc), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_page(doc, 595.0, 842.0, &page), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_add_text(doc, page, 72.0, 700.0, liberation, 12.0, "Sans"),
           OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_document_save_bytes(doc, &data, &len), OW_STATUS_OK, NULL);
    CHECK(contains(data, len, "q\nBT\n/F1 12 Tf\n1 0 0.3333 1 72 700 Tm\n"));
    CHECK(contains(data, len, "Tj\nET\nQ\n"));
    ow_bytes_free(data, len);
    data = NULL;
    ow_pdf_document_free(doc);
    doc = NULL;
    ow_font_resolver_free(resolver);
    ow_pdf_font_free(liberation);

    /* Only its Bold face: regular text comes out bold, and the report says so. */
    EXPECT(ow_font_book_new(&book), OW_STATUS_OK, NULL);
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Bold.ttf", fonts);
    EXPECT(ow_font_book_add_file(book, path, &added), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_new(book, &resolver), OW_STATUS_OK, NULL);
    ow_font_book_free(book);
    EXPECT(ow_pdf_font_resolve(resolver, "Liberation Sans", 0, 0, &bold), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_substitution(resolver, 0, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_RESOLUTION_EXACT);
    CHECK(message != NULL &&
          strstr(message, ", in bold: the family has no regular face installed") != NULL);
    ow_font_resolver_free(resolver);
    ow_pdf_font_free(bold);

    /* A font added from memory under another family name, as Word renames the subsets it
     * embeds, answers to that name and keeps its own. */
    EXPECT(ow_font_book_new(&book), OW_STATUS_OK, NULL);
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Regular.ttf", fonts);
    data = read_file(path, &len);
    CHECK(data != NULL);
    EXPECT(ow_font_book_add_bytes_as(book, data, len, "IBM Plex Serif Light", &added), OW_STATUS_OK,
           NULL);
    CHECK(added == 1);
    EXPECT(ow_font_book_add_bytes_as(book, data, len, NULL, &added), OW_STATUS_NULL_ARGUMENT,
           "family is null");
    EXPECT(ow_font_book_add_bytes_as(book, NULL, 0, "Any", &added), OW_STATUS_NULL_ARGUMENT,
           "data is null");
    EXPECT(ow_font_book_add_bytes_as(book, (const uint8_t *)"junk", 4, "Any", &added),
           OW_STATUS_MALFORMED, "not a TrueType");
    CHECK(added == 0);
    free(data);
    snprintf(path, sizeof path, "%s/liberation/LiberationSans-Bold.ttf", fonts);
    EXPECT(ow_font_book_add_file_as(book, path, "Plex File", &added), OW_STATUS_OK, NULL);
    CHECK(added == 1);
    EXPECT(ow_font_book_add_file_as(book, path, NULL, &added), OW_STATUS_NULL_ARGUMENT,
           "family is null");
    EXPECT(ow_font_resolver_new(book, &resolver), OW_STATUS_OK, NULL);
    ow_font_book_free(book);
    EXPECT(ow_pdf_font_resolve(resolver, "IBM Plex Serif Light", 0, 0, &liberation), OW_STATUS_OK,
           NULL);
    EXPECT(ow_pdf_font_resolve(resolver, "Liberation Sans", 0, 0, &arial), OW_STATUS_OK, NULL);
    EXPECT(ow_pdf_font_resolve(resolver, "Plex File", 1, 0, &bold), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_substitution_count(resolver, &count), OW_STATUS_OK, NULL);
    CHECK(count == 0);
    ow_font_resolver_free(resolver);
    ow_pdf_font_free(liberation);
    ow_pdf_font_free(arial);
    ow_pdf_font_free(bold);

    /* An empty book has nothing to stand in for a family. */
    EXPECT(ow_font_book_new(&empty), OW_STATUS_OK, NULL);
    EXPECT(ow_font_resolver_new(empty, &nothing), OW_STATUS_OK, NULL);
    not_found = (OwPdfFont *)(void *)&count;
    EXPECT(ow_pdf_font_resolve(nothing, "Calibri", 0, 0, &not_found), OW_STATUS_UNSUPPORTED,
           "no font found for family \"Calibri\"");
    CHECK(not_found == NULL);
    EXPECT(ow_font_resolver_substitution(nothing, 0, &kind, &message), OW_STATUS_OK, NULL);
    CHECK(kind == OW_RESOLUTION_MISSING);
    ow_font_resolver_free(nothing);

    /* A file that is not a font is skipped and listed. */
    snprintf(path, sizeof path, "%s/broken.ttf", dir);
    file = fopen(path, "wb");
    CHECK(file != NULL);
    if (file != NULL) {
        fputs("not a font", file);
        fclose(file);
    }
    EXPECT(ow_font_book_add_dir(empty, dir, &added), OW_STATUS_OK, NULL);
    EXPECT(ow_font_book_skipped_count(empty, &count), OW_STATUS_OK, NULL);
    CHECK(count >= 1);
    EXPECT(ow_font_book_skipped(empty, 0, &message), OW_STATUS_OK, NULL);
    CHECK(message != NULL && strstr(message, "broken.ttf") != NULL);
    snprintf(path, sizeof path, "%s/no-such-folder", dir);
    EXPECT(ow_font_book_add_dir(empty, path, &added), OW_STATUS_IO, "cannot read ");
    CHECK(added == 0);
    EXPECT(ow_font_book_add_bytes(empty, NULL, 0, &added), OW_STATUS_NULL_ARGUMENT, "data is null");
    EXPECT(ow_font_book_add_bytes(empty, (const uint8_t *)"junk", 4, &added),
           OW_STATUS_MALFORMED, "not a TrueType");
    /* Calls that change nothing leave the message valid (AddressSanitizer would report a
     * read of freed memory here). */
    CHECK(message != NULL && strstr(message, "broken.ttf") != NULL);
    ow_font_book_free(empty);
    ow_font_book_free(NULL);
    ow_font_resolver_free(NULL);
}

/* Converts the repository's smallest Word document to PDF with the fonts of tests/fonts
 * alone, in memory and to a file, reads the report, and checks the conversion's errors:
 * a page limit, a damaged document, NULL arguments. The documents are found from
 * `fonts`, the repository's tests/fonts. */
static void docx_to_pdf(const char *fonts, const char *out_dir) {
    char hello[1024];
    char pages[1024];
    char out[1024];
    char unwritable[1024];
    char carlito[1024];
    OwRenderOptions *from_file = NULL;
    OwRenderOptions *from_memory = NULL;
    uint8_t *carlito_data = NULL;
    size_t carlito_len = 0;
    OwRenderOptions *options = NULL;
    OwDocDocument *doc = NULL;
    OwDocDocument *from_bytes = NULL;
    OwDocDocument *long_doc = NULL;
    OwDocDocument *broken = NULL;
    OwLayoutReport *report = NULL;
    uint8_t *data = NULL;
    uint8_t *docx = NULL;
    size_t len = 0;
    size_t docx_len = 0;
    size_t count = 0;
    size_t i;
    int32_t kind = -1;
    int substituted = 0;
    const char *location = NULL;
    const char *message = NULL;

    snprintf(hello, sizeof hello, "%s/../../crates/officewright-doc/tests/data/hello.docx", fonts);
    snprintf(pages, sizeof pages, "%s/../corpus/word-rules/E-space-before-top.docx", fonts);
    snprintf(out, sizeof out, "%s/hello.pdf", out_dir);

    EXPECT(ow_render_options_new(&options), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_set_system_fonts(options, 0), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_add_font_dir(options, fonts), OW_STATUS_OK, NULL);
    EXPECT(ow_doc_document_open_path(hello, &doc), OW_STATUS_OK, NULL);

    /* In memory, with a report: Calibri, which tests/fonts lacks, becomes Carlito. */
    EXPECT(ow_doc_document_save_pdf_bytes(doc, options, &data, &len, &report), OW_STATUS_OK,
           NULL);
    CHECK(len > 5 && memcmp(data, "%PDF-", 5) == 0);
    EXPECT(ow_layout_report_len(report, &count), OW_STATUS_OK, NULL);
    CHECK(count > 0);
    for (i = 0; i < count; i++) {
        EXPECT(ow_layout_report_entry(report, i, &kind, &location, &message), OW_STATUS_OK,
               NULL);
        CHECK(location != NULL && message != NULL && message[0] != '\0');
        if (kind == OW_REPORT_KIND_FONT_SUBSTITUTED && strstr(message, "Carlito") != NULL &&
            strcmp(location, "document") == 0) {
            substituted = 1;
        }
    }
    CHECK(substituted);
    EXPECT(ow_layout_report_entry(report, count, &kind, &location, &message),
           OW_STATUS_INVALID_ARGUMENT, "out of range");
    CHECK(kind == OW_REPORT_KIND_OTHER && location == NULL && message == NULL);
    ow_layout_report_free(report);

    /* To a file, without a report: the same bytes. */
    EXPECT(ow_doc_document_save_pdf_path(doc, out, options, NULL), OW_STATUS_OK, NULL);
    CHECK(file_equals(out, data, len));
    ow_bytes_free(data, len);

    /* To a file with a report, and to one that cannot be written: the report reset. */
    report = NULL;
    EXPECT(ow_doc_document_save_pdf_path(doc, out, options, &report), OW_STATUS_OK, NULL);
    EXPECT(ow_layout_report_len(report, &count), OW_STATUS_OK, NULL);
    CHECK(count > 0);
    ow_layout_report_free(report);
    snprintf(unwritable, sizeof unwritable, "%s/missing-dir/hello.pdf", out_dir);
    report = (OwLayoutReport *)1;
    EXPECT(ow_doc_document_save_pdf_path(doc, unwritable, options, &report), OW_STATUS_IO,
           "missing-dir");
    CHECK(report == NULL);

    /* A font file by its path: Carlito alone stands in for Calibri. */
    snprintf(carlito, sizeof carlito, "%s/carlito/Carlito-Regular.ttf", fonts);
    EXPECT(ow_render_options_new(&from_file), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_set_system_fonts(from_file, 0), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_add_font_file(from_file, carlito), OW_STATUS_OK, NULL);
    EXPECT(ow_doc_document_save_pdf_bytes(doc, from_file, &data, &len, NULL), OW_STATUS_OK,
           NULL);
    CHECK(contains(data, len, "Carlito-Regular"));
    ow_bytes_free(data, len);
    EXPECT(ow_render_options_add_font_file(from_file, unwritable), OW_STATUS_IO, "cannot read");
    /* Data or a file that is not a font: refused at once, the file by its path. */
    EXPECT(ow_render_options_add_font_bytes(from_file, (const uint8_t *)"no font", 7),
           OW_STATUS_MALFORMED, "not a TrueType");
    EXPECT(ow_render_options_add_font_file(from_file, hello), OW_STATUS_MALFORMED, "hello.docx");
    ow_render_options_free(from_file);

    /* The same font from memory. */
    EXPECT(ow_render_options_new(&from_memory), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_set_system_fonts(from_memory, 0), OW_STATUS_OK, NULL);
    carlito_data = read_file(carlito, &carlito_len);
    CHECK(carlito_data != NULL);
    if (carlito_data != NULL) {
        EXPECT(ow_render_options_add_font_bytes(from_memory, carlito_data, carlito_len),
               OW_STATUS_OK, NULL);
        free(carlito_data);
    }
    EXPECT(ow_doc_document_save_pdf_bytes(doc, from_memory, &data, &len, NULL), OW_STATUS_OK,
           NULL);
    CHECK(contains(data, len, "Carlito-Regular"));
    ow_bytes_free(data, len);
    ow_render_options_free(from_memory);

    /* From memory: the document opens the same. */
    docx = read_file(hello, &docx_len);
    CHECK(docx != NULL);
    if (docx != NULL) {
        EXPECT(ow_doc_document_open_bytes(docx, docx_len, &from_bytes), OW_STATUS_OK, NULL);
        free(docx);
    }
    EXPECT(ow_doc_document_save_pdf_bytes(from_bytes, options, &data, &len, NULL), OW_STATUS_OK,
           NULL);
    CHECK(len > 5 && memcmp(data, "%PDF-", 5) == 0);
    ow_bytes_free(data, len);
    ow_doc_document_free(from_bytes);

    /* A document of several pages, past a limit of one: no PDF, no report. */
    EXPECT(ow_doc_document_open_path(pages, &long_doc), OW_STATUS_OK, NULL);
    EXPECT(ow_render_options_set_max_pages(options, 1), OW_STATUS_OK, NULL);
    report = (OwLayoutReport *)1;
    EXPECT(ow_doc_document_save_pdf_bytes(long_doc, options, &data, &len, &report),
           OW_STATUS_LIMIT_EXCEEDED, "pages");
    CHECK(data == NULL && len == 0 && report == NULL);
    EXPECT(ow_render_options_set_max_pages(options, 0), OW_STATUS_INVALID_ARGUMENT, "max_pages");
    ow_doc_document_free(long_doc);

    /* Damaged input and bad arguments. */
    broken = (OwDocDocument *)1;
    EXPECT(ow_doc_document_open_bytes((const uint8_t *)"not a docx", 10, &broken),
           OW_STATUS_MALFORMED, "");
    CHECK(broken == NULL);
    broken = (OwDocDocument *)1;
    EXPECT(ow_doc_document_open_path(NULL, &broken), OW_STATUS_NULL_ARGUMENT, "path is null");
    CHECK(broken == NULL);
    EXPECT(ow_render_options_set_system_fonts(options, 2), OW_STATUS_INVALID_ARGUMENT,
           "system_fonts is 2");
    EXPECT(ow_render_options_set_compression(options, 9), OW_STATUS_INVALID_ARGUMENT,
           "compression is 9");
    EXPECT(ow_doc_document_save_pdf_bytes(NULL, options, &data, &len, NULL),
           OW_STATUS_NULL_ARGUMENT, "doc is null");
    EXPECT(ow_doc_document_save_pdf_path(doc, out, NULL, NULL), OW_STATUS_NULL_ARGUMENT,
           "options is null");
    EXPECT(ow_layout_report_len(NULL, &count), OW_STATUS_NULL_ARGUMENT, "report is null");

    ow_doc_document_free(doc);
    ow_doc_document_free(NULL);
    ow_render_options_free(options);
    ow_render_options_free(NULL);
    ow_layout_report_free(NULL);
}

int main(int argc, char **argv) {
    uint32_t version = ow_abi_version();
    if (argc != 3) {
        fprintf(stderr, "usage: abi_test <directory for output files> <tests/fonts>\n");
        return 2;
    }
    if (version != OW_ABI_VERSION) {
        fprintf(stderr, "ow_abi_version() returned %lu, officewright.h has %lu\n",
                (unsigned long)version, (unsigned long)OW_ABI_VERSION);
        return 1;
    }
    build_and_save(argv[1]);
    text_layout_and_width();
    compression();
    errors();
    embedded_fonts(argv[2]);
    fonts_by_name(argv[2], argv[1]);
    docx_to_pdf(argv[2], argv[1]);
    if (failures != 0) {
        fprintf(stderr, "%d checks failed\n", failures);
        return 1;
    }
    return 0;
}
