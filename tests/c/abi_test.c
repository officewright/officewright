/* Calls every function of the C ABI through the static library and checks statuses,
 * error messages and out-parameters. Built with AddressSanitizer it also proves that
 * handles and buffers are released exactly once. Compiled as C and as C++ so the
 * header is proven usable from both.
 *
 * Usage: abi_test <directory for output files> */
#define _CRT_SECURE_NO_WARNINGS /* MSVC: plain fopen is what this test needs */
#include <stdio.h>
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

static void errors(void) {
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

int main(int argc, char **argv) {
    uint32_t version = ow_abi_version();
    if (argc != 2) {
        fprintf(stderr, "usage: abi_test <directory for output files>\n");
        return 2;
    }
    if (version != OW_ABI_VERSION) {
        fprintf(stderr, "ow_abi_version() returned %lu, officewright.h has %lu\n",
                (unsigned long)version, (unsigned long)OW_ABI_VERSION);
        return 1;
    }
    build_and_save(argv[1]);
    text_layout_and_width();
    errors();
    if (failures != 0) {
        fprintf(stderr, "%d checks failed\n", failures);
        return 1;
    }
    return 0;
}
