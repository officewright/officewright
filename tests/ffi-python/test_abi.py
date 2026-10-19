"""Loads the shared library with ctypes, as a binding with no build step would.

OW_LIB must point at the shared library (scripts/check-capi.sh sets it).
"""

import ctypes
import os
import re
import tempfile
import unittest
import zlib
from ctypes import POINTER, byref, c_char_p, c_double, c_int, c_size_t, c_uint8, c_uint32, c_void_p
from pathlib import Path

# The ABI version these tests were written against, as a binding would pin it.
EXPECTED_ABI_VERSION = 1

# OwStatus values, the standard font and the embedding notes from officewright.h.
OK, NULL_ARGUMENT, INVALID_UTF8, INVALID_ARGUMENT, IO, UNSUPPORTED, PANIC, MALFORMED, LIMIT_EXCEEDED = range(9)
OW_STANDARD_FONT_HELVETICA = 0
(
    OW_FONT_NOTE_OTHER,
    OW_FONT_NOTE_PREVIEW_AND_PRINT,
    OW_FONT_NOTE_EDITABLE,
    OW_FONT_NOTE_WHOLE_FONT,
    OW_FONT_NOTE_DEFAULT_INSTANCE,
) = range(5)
(
    OW_RESOLUTION_OTHER,
    OW_RESOLUTION_EXACT,
    OW_RESOLUTION_ALIAS,
    OW_RESOLUTION_OTHER_NAME,
    OW_RESOLUTION_VARIABLE_INSTANCE,
    OW_RESOLUTION_METRIC_COMPATIBLE,
    OW_RESOLUTION_FALLBACK,
    OW_RESOLUTION_MISSING,
) = range(8)
OW_COMPRESSION_NONE, OW_COMPRESSION_FLATE = range(2)
(
    OW_REPORT_KIND_OTHER,
    OW_REPORT_KIND_FONT_SUBSTITUTED,
    OW_REPORT_KIND_GLYPH_MISSING,
    OW_REPORT_KIND_FEATURE_UNSUPPORTED,
    OW_REPORT_KIND_FIELD_CACHED,
    OW_REPORT_KIND_COMPAT_IGNORED,
    OW_REPORT_KIND_VALUE_CLAMPED,
    OW_REPORT_KIND_OVERFLOW,
    OW_REPORT_KIND_FONT_EMBEDDING,
) = range(9)

# The OFL test fonts at the repository root.
FONTS = Path(__file__).resolve().parent.parent / "fonts"
LIBERATION_SANS = FONTS / "liberation" / "LiberationSans-Regular.ttf"
NOTO_SANS = FONTS / "noto" / "NotoSans-Regular.otf"

# Word documents in the repository: "Hello, world" in Calibri, and one of several pages.
REPOSITORY = FONTS.parent.parent
HELLO_DOCX = REPOSITORY / "crates" / "officewright-doc" / "tests" / "data" / "hello.docx"
PAGES_DOCX = REPOSITORY / "tests" / "corpus" / "word-rules" / "E-space-before-top.docx"

lib = ctypes.CDLL(os.environ["OW_LIB"])


def declare(name, restype, *argtypes):
    function = getattr(lib, name)
    function.restype = restype
    function.argtypes = list(argtypes)


declare("ow_abi_version", c_uint32)
declare("ow_last_error_message", c_char_p)
declare("ow_bytes_free", None, POINTER(c_uint8), c_size_t)
declare("ow_pdf_document_new", c_int, POINTER(c_void_p))
declare("ow_pdf_document_from_text", c_int, c_char_p, c_double, c_double, c_void_p, c_double, POINTER(c_void_p))
declare("ow_pdf_document_add_page", c_int, c_void_p, c_double, c_double, POINTER(c_size_t))
declare("ow_pdf_document_add_text", c_int, c_void_p, c_size_t, c_double, c_double, c_void_p, c_double, c_char_p)
declare("ow_pdf_document_set_compression", c_int, c_void_p, ctypes.c_int32)
declare("ow_pdf_document_set_subset_budget", c_int, c_void_p, ctypes.c_uint64)
declare("ow_pdf_document_save_path", c_int, c_void_p, c_char_p)
declare("ow_pdf_document_save_bytes", c_int, c_void_p, POINTER(POINTER(c_uint8)), POINTER(c_size_t))
declare("ow_pdf_document_free", None, c_void_p)
declare("ow_pdf_font_new_standard", c_int, ctypes.c_int32, POINTER(c_void_p))
declare("ow_pdf_font_open_path", c_int, c_char_p, c_uint32, POINTER(c_void_p))
declare("ow_pdf_font_open_bytes", c_int, c_char_p, c_size_t, c_uint32, POINTER(c_void_p))
declare("ow_pdf_font_note_count", c_int, c_void_p, POINTER(c_size_t))
declare("ow_pdf_font_note", c_int, c_void_p, c_size_t, POINTER(ctypes.c_int32), POINTER(c_char_p))
declare("ow_pdf_font_text_width", c_int, c_void_p, c_char_p, c_double, POINTER(c_double))
declare("ow_pdf_font_free", None, c_void_p)
declare("ow_font_book_new", c_int, POINTER(c_void_p))
declare("ow_font_book_add_system_fonts", c_int, c_void_p, POINTER(c_size_t))
declare("ow_font_book_add_dir", c_int, c_void_p, c_char_p, POINTER(c_size_t))
declare("ow_font_book_add_file", c_int, c_void_p, c_char_p, POINTER(c_size_t))
declare("ow_font_book_add_bytes", c_int, c_void_p, c_char_p, c_size_t, POINTER(c_size_t))
declare("ow_font_book_add_bytes_as", c_int, c_void_p, c_char_p, c_size_t, c_char_p, POINTER(c_size_t))
declare("ow_font_book_add_file_as", c_int, c_void_p, c_char_p, c_char_p, POINTER(c_size_t))
declare("ow_font_book_face_count", c_int, c_void_p, POINTER(c_size_t))
declare("ow_font_book_skipped_count", c_int, c_void_p, POINTER(c_size_t))
declare("ow_font_book_skipped", c_int, c_void_p, c_size_t, POINTER(c_char_p))
declare("ow_font_book_free", None, c_void_p)
declare("ow_font_resolver_new", c_int, c_void_p, POINTER(c_void_p))
declare("ow_font_resolver_substitution_count", c_int, c_void_p, POINTER(c_size_t))
declare("ow_font_resolver_substitution", c_int, c_void_p, c_size_t, POINTER(ctypes.c_int32), POINTER(c_char_p))
declare("ow_font_resolver_free", None, c_void_p)
declare("ow_pdf_font_resolve", c_int, c_void_p, c_char_p, ctypes.c_int32, ctypes.c_int32, POINTER(c_void_p))
declare("ow_doc_document_open_path", c_int, c_char_p, POINTER(c_void_p))
declare("ow_doc_document_open_bytes", c_int, c_char_p, c_size_t, POINTER(c_void_p))
declare("ow_doc_document_save_pdf_path", c_int, c_void_p, c_char_p, c_void_p, POINTER(c_void_p))
declare(
    "ow_doc_document_save_pdf_bytes",
    c_int,
    c_void_p,
    c_void_p,
    POINTER(POINTER(c_uint8)),
    POINTER(c_size_t),
    POINTER(c_void_p),
)
declare("ow_doc_document_free", None, c_void_p)
declare("ow_render_options_new", c_int, POINTER(c_void_p))
declare("ow_render_options_set_max_pages", c_int, c_void_p, c_size_t)
declare("ow_render_options_set_system_fonts", c_int, c_void_p, ctypes.c_int32)
declare("ow_render_options_add_font_dir", c_int, c_void_p, c_char_p)
declare("ow_render_options_add_font_bytes", c_int, c_void_p, c_char_p, c_size_t)
declare("ow_render_options_add_font_file", c_int, c_void_p, c_char_p)
declare("ow_render_options_set_compression", c_int, c_void_p, ctypes.c_int32)
declare("ow_render_options_free", None, c_void_p)
declare("ow_layout_report_len", c_int, c_void_p, POINTER(c_size_t))
declare("ow_layout_report_entry", c_int, c_void_p, c_size_t, POINTER(ctypes.c_int32), POINTER(c_char_p), POINTER(c_char_p))
declare("ow_layout_report_free", None, c_void_p)


def with_fs_type(font, fs_type):
    """The font with fsType in its OS/2 table, what its license allows, set to fs_type."""
    font = bytearray(font)
    tables = int.from_bytes(font[4:6], "big")
    for record in range(12, 12 + 16 * tables, 16):
        if font[record : record + 4] == b"OS/2":
            os2 = int.from_bytes(font[record + 8 : record + 12], "big")
            font[os2 + 8 : os2 + 10] = fs_type.to_bytes(2, "big")
            return bytes(font)
    raise ValueError("no OS/2 table")


def with_table(font, tag, data):
    """The font with table `tag` added: its table directory, sorted by tag as it must be,
    gets a record more, and every table keeps its data."""
    count = int.from_bytes(font[4:6], "big")
    tables = {}
    for record in range(12, 12 + 16 * count, 16):
        offset = int.from_bytes(font[record + 8 : record + 12], "big")
        length = int.from_bytes(font[record + 12 : record + 16], "big")
        tables[font[record : record + 4]] = font[offset : offset + length]
    tables[tag] = data
    selector = len(tables).bit_length() - 1
    search = 16 << selector
    fields = (len(tables), search, selector, 16 * len(tables) - search)
    directory = bytearray(font[:4] + b"".join(field.to_bytes(2, "big") for field in fields))
    body = bytearray()
    for tag in sorted(tables):
        table = tables[tag] + bytes(-len(tables[tag]) % 4)
        checksum = sum(int.from_bytes(table[at : at + 4], "big") for at in range(0, len(table), 4))
        offset = 12 + 16 * len(tables) + len(body)
        directory += tag + b"".join(v.to_bytes(4, "big") for v in (checksum & 0xFFFFFFFF, offset, len(tables[tag])))
        body += table
    return bytes(directory + body)


# The fvar table of a variable font with one axis, weight from 100 to 900, its default
# 400, named by name ID 2; it has no named instances.
WEIGHT_AXIS = (
    b"".join(field.to_bytes(2, "big") for field in (1, 0, 16, 2, 1, 20, 0, 8))
    + b"wght"
    + b"".join((value << 16).to_bytes(4, "big") for value in (100, 400, 900))
    + b"\0\0\0\2"
)


def last_error():
    return lib.ow_last_error_message().decode("utf-8")


class AbiTest(unittest.TestCase):
    def check(self, status, expected=OK):
        self.assertEqual(status, expected, last_error())

    def new_document(self):
        doc = c_void_p()
        self.check(lib.ow_pdf_document_new(byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        return doc

    def helvetica(self):
        font = c_void_p()
        self.check(lib.ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, byref(font)))
        self.addCleanup(lib.ow_pdf_font_free, font)
        return font

    def save_bytes(self, doc):
        data = POINTER(c_uint8)()
        length = c_size_t()
        self.check(lib.ow_pdf_document_save_bytes(doc, byref(data), byref(length)))
        try:
            return ctypes.string_at(data, length.value)
        finally:
            lib.ow_bytes_free(data, length)

    def test_abi_version(self):
        self.assertEqual(lib.ow_abi_version(), EXPECTED_ABI_VERSION)

    def test_builds_a_document_and_saves_it_to_memory_and_to_a_path(self):
        doc = self.new_document()
        page = c_size_t(99)
        self.check(lib.ow_pdf_document_add_page(doc, 595.0, 842.0, byref(page)))
        self.assertEqual(page.value, 0)
        font = c_void_p()
        self.check(lib.ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, byref(font)))
        self.check(lib.ow_pdf_document_add_text(doc, page, 72.0, 770.0, font, 12.0, "Grüße (€5)".encode()))
        # The document keeps what it needs, so the font can go before the document.
        lib.ow_pdf_font_free(font)
        self.assertEqual(last_error(), "")
        # Uncompressed, to read the page content.
        self.check(lib.ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE))

        pdf = self.save_bytes(doc)
        self.assertTrue(pdf.startswith(b"%PDF-1.4\n"))
        self.assertTrue(pdf.endswith(b"%%EOF\n"))
        self.assertIn(rb"(Gr\374\337e \(\2005\)) Tj", pdf)

        with tempfile.TemporaryDirectory() as directory:
            # A non-ASCII name: on Windows the UTF-8 path must reach the UTF-16 file API.
            path = Path(directory) / "отчёт – Grüße.pdf"
            self.check(lib.ow_pdf_document_save_path(doc, str(path).encode()))
            self.assertEqual(path.read_bytes(), pdf)

    def test_lays_out_text_and_measures_it(self):
        font = self.helvetica()
        doc = c_void_p()
        self.check(lib.ow_pdf_document_from_text(b"Hello,\nworld\fpage two", 612.0, 792.0, font, 11.0, byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        self.check(lib.ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE))
        pdf = self.save_bytes(doc)
        self.assertIn(b"/Count 2", pdf)
        self.assertIn(b"(page two) Tj", pdf)

        width = c_double()
        self.check(lib.ow_pdf_font_text_width(font, b"H", 10.0, byref(width)))
        self.assertAlmostEqual(width.value, 7.22)

    def test_compresses_page_content_unless_told_not_to(self):
        font = self.helvetica()
        doc = c_void_p()
        self.check(lib.ow_pdf_document_from_text(b"Hello, world\n" * 40, 595.0, 842.0, font, 11.0, byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        compressed = self.save_bytes(doc)
        self.check(lib.ow_pdf_document_set_compression(doc, OW_COMPRESSION_NONE))
        plain = self.save_bytes(doc)
        self.assertNotIn(b"/Filter", plain)
        self.assertIn(b"(Hello, world) Tj", plain)

        # The page content is object 5 in both; compressed, it inflates to the plain one.
        def content(pdf):
            match = re.search(rb"\n5 0 obj\n<< /Length (\d+)( /Filter /FlateDecode)? >>\nstream\n", pdf)
            self.assertIsNotNone(match)
            return match.group(2), pdf[match.end() : match.end() + int(match.group(1))]

        flate, data = content(compressed)
        self.assertEqual(flate, b" /Filter /FlateDecode")
        self.assertEqual(content(plain), (None, zlib.decompress(data)))

        self.check(lib.ow_pdf_document_set_compression(doc, 2), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "compression is 2, not OW_COMPRESSION_NONE (0) or OW_COMPRESSION_FLATE (1)")
        self.assertEqual(self.save_bytes(doc), plain)
        self.check(lib.ow_pdf_document_set_compression(doc, OW_COMPRESSION_FLATE))
        self.assertEqual(self.save_bytes(doc), compressed)
        self.check(lib.ow_pdf_document_set_compression(None, OW_COMPRESSION_NONE), NULL_ARGUMENT)

    def test_reports_errors_with_a_status_and_a_message(self):
        doc = self.new_document()
        font = self.helvetica()
        page = c_size_t()
        self.check(lib.ow_pdf_document_new(None), NULL_ARGUMENT)
        self.assertEqual(last_error(), "out_doc is null")
        # Failing calls must reset their out-parameters, so these start non-NULL.
        unknown = c_void_p(1)
        self.check(lib.ow_pdf_font_new_standard(7, byref(unknown)), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "standard font 7 is not one of the OW_STANDARD_FONT_* values")
        self.assertFalse(unknown)
        page.value = 99
        self.check(lib.ow_pdf_document_add_page(doc, 1.0, 842.0, byref(page)), INVALID_ARGUMENT)
        self.assertIn("outside the 3 to 14400 pt range", last_error())
        self.assertEqual(page.value, 0)
        not_made = c_void_p(1)
        self.check(lib.ow_pdf_document_from_text(b"x", 612.0, 792.0, None, 11.0, byref(not_made)), NULL_ARGUMENT)
        self.assertFalse(not_made)
        self.check(lib.ow_pdf_document_add_page(doc, 595.0, 842.0, byref(page)))

        self.check(lib.ow_pdf_document_add_text(doc, 1, 0.0, 0.0, font, 10.0, b"x"), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "page index 1 is out of range for a document with 1 pages")
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, None), NULL_ARGUMENT)
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, b"ab\xff"), INVALID_UTF8)
        self.assertEqual(last_error(), "text is not valid UTF-8 at byte 2")
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, font, 10.0, "Жар".encode()), UNSUPPORTED)
        self.assertEqual(last_error(), "character U+0416 'Ж' at byte 0 is not in WinAnsiEncoding")
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, None, 10.0, b"x"), NULL_ARGUMENT)
        self.assertEqual(last_error(), "font is null")

        with tempfile.TemporaryDirectory() as directory:
            missing = str(Path(directory) / "missing" / "out.pdf")
            self.check(lib.ow_pdf_document_save_path(doc, missing.encode()), IO)
            self.assertTrue(last_error().startswith(f"cannot write {missing}: "), last_error())

        empty = self.new_document()
        data = ctypes.cast(c_void_p(1), POINTER(c_uint8))
        length = c_size_t(7)
        self.check(lib.ow_pdf_document_save_bytes(empty, byref(data), byref(length)), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "document has no pages")
        self.assertFalse(data)
        self.assertEqual(length.value, 0)

        lib.ow_pdf_document_free(None)
        lib.ow_pdf_font_free(None)
        lib.ow_bytes_free(None, 0)

    def open_font(self, data):
        font = c_void_p()
        self.check(lib.ow_pdf_font_open_bytes(data, len(data), 0, byref(font)))
        self.addCleanup(lib.ow_pdf_font_free, font)
        return font

    def notes(self, font):
        count = c_size_t(99)
        self.check(lib.ow_pdf_font_note_count(font, byref(count)))
        notes = []
        for index in range(count.value):
            kind = ctypes.c_int32(-1)
            message = c_char_p()
            self.check(lib.ow_pdf_font_note(font, index, byref(kind), byref(message)))
            notes.append((kind.value, message.value.decode("utf-8")))
        return notes

    def test_embeds_fonts_from_a_path_and_from_memory(self):
        liberation = c_void_p()
        self.check(lib.ow_pdf_font_open_path(str(LIBERATION_SANS).encode(), 0, byref(liberation)))
        self.addCleanup(lib.ow_pdf_font_free, liberation)
        noto = self.open_font(NOTO_SANS.read_bytes())
        self.assertEqual(self.notes(liberation), [])

        doc = c_void_p()
        self.check(lib.ow_pdf_document_from_text("Привет, мир".encode(), 595.0, 842.0, liberation, 12.0, byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        self.check(lib.ow_pdf_document_add_text(doc, 0, 72.0, 600.0, noto, 12.0, "Καλημέρα".encode()))
        pdf = self.save_bytes(doc)
        self.assertIn(b"/FontFile2", pdf)
        self.assertIn(b"/Subtype /CIDFontType0C", pdf)
        self.assertEqual(pdf.count(b"/ToUnicode"), 2)

        # Reducing Noto Sans (CFF) to its glyphs reads its charstrings, which a budget of 0
        # does not allow; a large one does.
        self.check(lib.ow_pdf_document_set_subset_budget(doc, 0))
        placeholder = ctypes.create_string_buffer(1)
        data = ctypes.cast(placeholder, POINTER(c_uint8))
        size = c_size_t(99)
        self.check(lib.ow_pdf_document_save_bytes(doc, byref(data), byref(size)), LIMIT_EXCEEDED)
        self.assertIn("hint mask bytes left of its budget", last_error())
        self.assertFalse(data)
        self.assertEqual(size.value, 0)
        self.check(lib.ow_pdf_document_set_subset_budget(doc, 2**64 - 1))
        self.assertEqual(self.save_bytes(doc), pdf)
        self.check(lib.ow_pdf_document_set_subset_budget(None, 0), NULL_ARGUMENT)
        self.assertEqual(last_error(), "doc is null")

        width = c_double()
        self.check(lib.ow_pdf_font_text_width(noto, "Жж".encode(), 10.0, byref(width)))
        self.assertGreater(width.value, 0.0)
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, liberation, 10.0, "aש".encode()), UNSUPPORTED)
        self.assertIn("is in the Hebrew script", last_error())
        self.check(lib.ow_pdf_document_add_text(doc, 0, 0.0, 0.0, liberation, 10.0, "漢".encode()), UNSUPPORTED)
        self.assertEqual(last_error(), "font LiberationSans has no glyph for character U+6F22 '漢' at byte 0")

    def test_lists_what_the_font_license_restricts(self):
        whole = self.open_font(with_fs_type(LIBERATION_SANS.read_bytes(), 0x0104))
        self.assertEqual(
            self.notes(whole),
            [
                (
                    OW_FONT_NOTE_PREVIEW_AND_PRINT,
                    "its license allows the embedded font only for viewing and printing the document",
                ),
                (OW_FONT_NOTE_WHOLE_FONT, "its license forbids subsetting, so the whole font is embedded"),
            ],
        )
        kind = ctypes.c_int32(-1)
        message = c_char_p(b"placeholder")
        self.check(lib.ow_pdf_font_note(whole, 2, byref(kind), byref(message)), INVALID_ARGUMENT)
        self.assertEqual(kind.value, OW_FONT_NOTE_OTHER)
        self.assertIsNone(message.value)

        restricted = with_fs_type(LIBERATION_SANS.read_bytes(), 0x0002)
        font = c_void_p(1)
        self.check(lib.ow_pdf_font_open_bytes(restricted, len(restricted), 0, byref(font)), UNSUPPORTED)
        self.assertEqual(last_error(), "the license of font LiberationSans does not allow embedding it")
        self.assertFalse(font)

    def test_notes_that_a_variable_font_is_embedded_as_its_default_instance(self):
        variable = self.open_font(with_table(LIBERATION_SANS.read_bytes(), b"fvar", WEIGHT_AXIS))
        self.assertEqual(
            self.notes(variable),
            [(OW_FONT_NOTE_DEFAULT_INSTANCE, "it is a variable font; only its default instance is embedded")],
        )
        doc = c_void_p()
        self.check(lib.ow_pdf_document_from_text(b"Weight", 595.0, 842.0, variable, 12.0, byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        self.assertIn(b"/FontFile2", self.save_bytes(doc))

    def test_reports_fonts_it_cannot_open(self):
        font = c_void_p(1)
        self.check(lib.ow_pdf_font_open_bytes(b"not a font", 10, 0, byref(font)), MALFORMED)
        self.assertTrue(last_error().startswith("cannot use the font data: data is not a TrueType"), last_error())
        self.assertFalse(font)
        # A face the file does not have is the caller's mistake, not damage.
        data = (FONTS / "liberation" / "LiberationSans-Regular.ttf").read_bytes()
        font = c_void_p(1)
        self.check(lib.ow_pdf_font_open_bytes(data, len(data), 1, byref(font)), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "cannot use the font data: font face 1 does not exist, the file has 1 face")
        self.assertFalse(font)
        missing = FONTS / "no-such-font.ttf"
        self.check(lib.ow_pdf_font_open_path(str(missing).encode(), 0, byref(font)), IO)
        self.assertTrue(last_error().startswith(f"cannot read {missing}: "), last_error())
        self.check(lib.ow_pdf_font_open_path(None, 0, byref(font)), NULL_ARGUMENT)
        self.assertEqual(last_error(), "path is null")
        self.check(lib.ow_pdf_font_open_bytes(None, 0, 0, byref(font)), NULL_ARGUMENT)
        self.assertEqual(last_error(), "data is null")


    def new_book(self):
        book = c_void_p()
        self.check(lib.ow_font_book_new(byref(book)))
        self.addCleanup(lib.ow_font_book_free, book)
        return book

    def resolver(self, book):
        resolver = c_void_p()
        self.check(lib.ow_font_resolver_new(book, byref(resolver)))
        self.addCleanup(lib.ow_font_resolver_free, resolver)
        return resolver

    def resolve(self, resolver, family, bold=0, italic=0, expected=OK):
        font = c_void_p(1)
        self.check(lib.ow_pdf_font_resolve(resolver, family.encode(), bold, italic, byref(font)), expected)
        if expected == OK:
            self.addCleanup(lib.ow_pdf_font_free, font)
        else:
            self.assertFalse(font)
        return font

    def substitutions(self, resolver):
        count = c_size_t(99)
        self.check(lib.ow_font_resolver_substitution_count(resolver, byref(count)))
        report = []
        for index in range(count.value):
            kind = ctypes.c_int32(-1)
            message = c_char_p()
            self.check(lib.ow_font_resolver_substitution(resolver, index, byref(kind), byref(message)))
            report.append((kind.value, message.value.decode("utf-8")))
        return report

    def test_chooses_fonts_by_family_from_the_test_fonts(self):
        # Only tests/fonts, not the system fonts, so the result is the same everywhere.
        book = self.new_book()
        added = c_size_t()
        self.check(lib.ow_font_book_add_dir(book, str(FONTS).encode(), byref(added)))
        # Carlito and Liberation Sans, Serif and Mono in four styles each, and Noto Sans.
        self.assertEqual(added.value, 17)
        resolver = self.resolver(book)

        self.resolve(resolver, "Liberation Sans")
        self.resolve(resolver, "noto sans")
        self.assertEqual(self.substitutions(resolver), [])
        arial = self.resolve(resolver, "Arial")
        # Liberation Sans has a bold face; Noto Sans has none, so its regular face is drawn bold.
        self.resolve(resolver, "Liberation Sans", bold=1)
        self.resolve(resolver, "Noto Sans", bold=1)
        self.assertEqual(
            self.substitutions(resolver),
            [
                (OW_RESOLUTION_METRIC_COMPATIBLE, '"Arial" → "Liberation Sans" (metric-compatible substitute)'),
                (OW_RESOLUTION_EXACT, '"Noto Sans" bold → "Noto Sans" with synthetic bold'),
            ],
        )
        doc = c_void_p()
        self.check(lib.ow_pdf_document_from_text("Привет".encode(), 595.0, 842.0, arial, 12.0, byref(doc)))
        self.addCleanup(lib.ow_pdf_document_free, doc)
        self.assertIn(b"+LiberationSans", self.save_bytes(doc))

    def test_styles_a_family_without_some_faces_as_word_does(self):
        # Liberation Sans without its Italic face: italic text slants Regular, as Word
        # does, rather than take Bold Italic.
        book = self.new_book()
        added = c_size_t()
        for style in ("Regular", "Bold", "BoldItalic"):
            path = FONTS / "liberation" / f"LiberationSans-{style}.ttf"
            self.check(lib.ow_font_book_add_file(book, str(path).encode(), byref(added)))
        resolver = self.resolver(book)
        self.resolve(resolver, "Liberation Sans", italic=1)
        self.resolve(resolver, "Liberation Sans", bold=1, italic=1)
        self.assertEqual(
            self.substitutions(resolver),
            [(OW_RESOLUTION_EXACT, '"Liberation Sans" italic → "Liberation Sans" with synthetic italic')],
        )
        # Only its Bold face: regular text comes out bold, and the report says so.
        book = self.new_book()
        bold = FONTS / "liberation" / "LiberationSans-Bold.ttf"
        self.check(lib.ow_font_book_add_file(book, str(bold).encode(), byref(added)))
        resolver = self.resolver(book)
        self.resolve(resolver, "Liberation Sans")
        self.assertEqual(
            self.substitutions(resolver),
            [
                (
                    OW_RESOLUTION_EXACT,
                    '"Liberation Sans" → "Liberation Sans", in bold: the family has no regular face installed',
                )
            ],
        )

    def test_reports_a_family_nothing_stands_in_for(self):
        resolver = self.resolver(self.new_book())
        self.resolve(resolver, "Calibri", expected=UNSUPPORTED)
        self.assertEqual(last_error(), 'no font found for family "Calibri"')
        self.assertEqual(self.substitutions(resolver), [(OW_RESOLUTION_MISSING, '"Calibri" → no font available')])
        self.resolve(resolver, "Calibri", bold=2, expected=INVALID_ARGUMENT)
        self.assertEqual(last_error(), "bold is 2, not 0 or 1")

    def test_adds_the_system_fonts_whatever_the_machine_has(self):
        added = c_size_t(0)
        self.check(lib.ow_font_book_add_system_fonts(self.new_book(), byref(added)))

    def test_lists_what_a_font_book_skipped(self):
        book = self.new_book()
        added = c_size_t(7)
        with tempfile.TemporaryDirectory() as directory:
            broken = Path(directory) / "broken.ttf"
            broken.write_bytes(b"not a font")
            self.check(lib.ow_font_book_add_dir(book, directory.encode(), byref(added)))
            self.assertEqual(added.value, 0)
            count = c_size_t()
            self.check(lib.ow_font_book_skipped_count(book, byref(count)))
            self.assertEqual(count.value, 1)
            message = c_char_p()
            self.check(lib.ow_font_book_skipped(book, 0, byref(message)))
            self.assertTrue(message.value.decode("utf-8").startswith(f"{broken}: data is not a TrueType"))
            self.check(lib.ow_font_book_add_dir(book, f"{directory}/missing".encode(), byref(added)), IO)
            self.assertEqual(added.value, 0)
            data = LIBERATION_SANS.read_bytes()
            self.check(lib.ow_font_book_add_bytes(book, data, len(data), byref(added)))
            self.assertEqual(added.value, 1)
            faces = c_size_t()
            self.check(lib.ow_font_book_face_count(book, byref(faces)))
            self.assertEqual(faces.value, 1)

    def test_adds_a_font_under_another_family_name(self):
        # As Word renames the subsets it embeds: the face answers to the name given and to
        # its own, and the report has no substitution.
        book = self.new_book()
        data = LIBERATION_SANS.read_bytes()
        added = c_size_t(7)
        self.check(lib.ow_font_book_add_bytes_as(book, data, len(data), "IBM Plex Serif Light".encode(), byref(added)))
        self.assertEqual(added.value, 1)
        self.check(lib.ow_font_book_add_bytes_as(book, data, len(data), b"\xff", byref(added)), INVALID_UTF8)
        self.assertEqual(added.value, 0)
        bold = str(LIBERATION_SANS).replace("Regular", "Bold").encode()
        self.check(lib.ow_font_book_add_file_as(book, bold, b"Plex File", byref(added)))
        self.assertEqual(added.value, 1)
        resolver = c_void_p()
        self.check(lib.ow_font_resolver_new(book, byref(resolver)))
        self.addCleanup(lib.ow_font_resolver_free, resolver)
        # The face added under "Plex File" is a bold one.
        for family, bold in ((b"IBM Plex Serif Light", 0), (b"Liberation Sans", 0), (b"Plex File", 1)):
            font = c_void_p()
            self.check(lib.ow_pdf_font_resolve(resolver, family, bold, 0, byref(font)))
            lib.ow_pdf_font_free(font)
        count = c_size_t(7)
        self.check(lib.ow_font_resolver_substitution_count(resolver, byref(count)))
        self.assertEqual(count.value, 0)


    def render_options(self):
        """Options with the fonts of tests/fonts alone."""
        options = c_void_p()
        self.check(lib.ow_render_options_new(byref(options)))
        self.addCleanup(lib.ow_render_options_free, options)
        self.check(lib.ow_render_options_set_system_fonts(options, 0))
        self.check(lib.ow_render_options_add_font_dir(options, str(FONTS).encode()))
        return options

    def open_docx(self, path):
        doc = c_void_p()
        self.check(lib.ow_doc_document_open_path(str(path).encode(), byref(doc)))
        self.addCleanup(lib.ow_doc_document_free, doc)
        return doc

    def test_converts_a_word_document_to_pdf_and_reports_what_differs(self):
        options = self.render_options()
        doc = self.open_docx(HELLO_DOCX)
        data = POINTER(c_uint8)()
        length = c_size_t()
        report = c_void_p()
        self.check(lib.ow_doc_document_save_pdf_bytes(doc, options, byref(data), byref(length), byref(report)))
        try:
            pdf = ctypes.string_at(data, length.value)
        finally:
            lib.ow_bytes_free(data, length)
        self.assertTrue(pdf.startswith(b"%PDF-"))
        # Calibri, which tests/fonts lacks, is laid out in Carlito, and the report says so.
        count = c_size_t()
        self.check(lib.ow_layout_report_len(report, byref(count)))
        entries = []
        for index in range(count.value):
            kind, location, message = ctypes.c_int32(), c_char_p(), c_char_p()
            self.check(lib.ow_layout_report_entry(report, index, byref(kind), byref(location), byref(message)))
            entries.append((kind.value, location.value.decode(), message.value.decode()))
        lib.ow_layout_report_free(report)
        self.assertIn(
            (OW_REPORT_KIND_FONT_SUBSTITUTED, "document", '"Calibri" \u2192 "Carlito" (metric-compatible substitute)'),
            entries,
        )
        # The same document from memory, written to a file without a report.
        docx = HELLO_DOCX.read_bytes()
        from_bytes = c_void_p()
        self.check(lib.ow_doc_document_open_bytes(docx, len(docx), byref(from_bytes)))
        self.addCleanup(lib.ow_doc_document_free, from_bytes)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "hello.pdf"
            self.check(lib.ow_doc_document_save_pdf_path(from_bytes, str(out).encode(), options, None))
            self.assertEqual(out.read_bytes(), pdf)
            # With a report; and to a folder that does not exist, the report reset.
            report = c_void_p()
            self.check(lib.ow_doc_document_save_pdf_path(from_bytes, str(out).encode(), options, byref(report)))
            count = c_size_t()
            self.check(lib.ow_layout_report_len(report, byref(count)))
            self.assertGreater(count.value, 0)
            lib.ow_layout_report_free(report)
            report = c_void_p(1)
            missing = Path(tmp) / "missing-dir" / "hello.pdf"
            self.check(lib.ow_doc_document_save_pdf_path(from_bytes, str(missing).encode(), options, byref(report)), IO)
            self.assertIsNone(report.value)

    def test_a_font_file_by_its_path(self):
        options = c_void_p()
        self.check(lib.ow_render_options_new(byref(options)))
        self.addCleanup(lib.ow_render_options_free, options)
        self.check(lib.ow_render_options_set_system_fonts(options, 0))
        carlito = FONTS / "carlito" / "Carlito-Regular.ttf"
        self.check(lib.ow_render_options_add_font_file(options, str(carlito).encode()))
        doc = self.open_docx(HELLO_DOCX)
        data = POINTER(c_uint8)()
        length = c_size_t()
        self.check(lib.ow_doc_document_save_pdf_bytes(doc, options, byref(data), byref(length), None))
        try:
            self.assertIn(b"Carlito-Regular", ctypes.string_at(data, length.value))
        finally:
            lib.ow_bytes_free(data, length)
        self.check(lib.ow_render_options_add_font_file(options, b"/nonexistent/font.ttf"), IO)
        self.assertTrue(last_error().startswith("cannot read /nonexistent/font.ttf"))

    def test_a_font_from_memory(self):
        options = c_void_p()
        self.check(lib.ow_render_options_new(byref(options)))
        self.addCleanup(lib.ow_render_options_free, options)
        self.check(lib.ow_render_options_set_system_fonts(options, 0))
        carlito = (FONTS / "carlito" / "Carlito-Regular.ttf").read_bytes()
        self.check(lib.ow_render_options_add_font_bytes(options, carlito, len(carlito)))
        doc = self.open_docx(HELLO_DOCX)
        data = POINTER(c_uint8)()
        length = c_size_t()
        self.check(lib.ow_doc_document_save_pdf_bytes(doc, options, byref(data), byref(length), None))
        try:
            self.assertIn(b"Carlito-Regular", ctypes.string_at(data, length.value))
        finally:
            lib.ow_bytes_free(data, length)

    def test_conversion_errors_have_a_status_and_a_message(self):
        options = self.render_options()
        # More pages than allowed: no PDF and no report.
        long_doc = self.open_docx(PAGES_DOCX)
        self.check(lib.ow_render_options_set_max_pages(options, 1))
        data = POINTER(c_uint8)()
        length = c_size_t()
        report = c_void_p(1)
        self.check(
            lib.ow_doc_document_save_pdf_bytes(long_doc, options, byref(data), byref(length), byref(report)),
            LIMIT_EXCEEDED,
        )
        self.assertIn("pages", last_error())
        self.assertFalse(data)
        self.assertEqual(length.value, 0)
        self.assertIsNone(report.value)
        # A damaged document, and options out of range.
        doc = c_void_p(1)
        self.check(lib.ow_doc_document_open_bytes(b"not a docx", 10, byref(doc)), MALFORMED)
        self.assertIsNone(doc.value)
        self.check(lib.ow_render_options_set_max_pages(options, 0), INVALID_ARGUMENT)
        self.check(lib.ow_render_options_set_system_fonts(options, 2), INVALID_ARGUMENT)
        self.assertEqual(last_error(), "system_fonts is 2, not 0 or 1")
        self.check(lib.ow_render_options_set_compression(options, 9), INVALID_ARGUMENT)
        self.check(lib.ow_doc_document_open_path(None, byref(doc)), NULL_ARGUMENT)
        self.assertEqual(last_error(), "path is null")
        # Font data or a file that is not a font is refused when it is added, and leaves
        # the options as they were.
        refused = c_void_p()
        self.check(lib.ow_render_options_new(byref(refused)))
        self.addCleanup(lib.ow_render_options_free, refused)
        self.check(lib.ow_render_options_set_system_fonts(refused, 0))
        self.check(lib.ow_render_options_add_font_bytes(refused, b"no font", 7), MALFORMED)
        self.check(lib.ow_render_options_add_font_file(refused, str(HELLO_DOCX).encode()), MALFORMED)
        self.assertTrue(last_error().startswith(f"{HELLO_DOCX}: "))
        hello = self.open_docx(HELLO_DOCX)
        self.check(lib.ow_doc_document_save_pdf_bytes(hello, refused, byref(data), byref(length), None))
        lib.ow_bytes_free(data, length)


if __name__ == "__main__":
    unittest.main()
