"""Loads the shared library with ctypes, as a binding with no build step would.

OW_LIB must point at the shared library (scripts/check-capi.sh sets it).
"""

import ctypes
import os
import tempfile
import unittest
from ctypes import POINTER, byref, c_char_p, c_double, c_int, c_size_t, c_uint8, c_uint32, c_void_p
from pathlib import Path

# The ABI version these tests were written against, as a binding would pin it.
EXPECTED_ABI_VERSION = 1

# OwStatus values and the standard font from officewright.h.
OK, NULL_ARGUMENT, INVALID_UTF8, INVALID_ARGUMENT, IO, UNSUPPORTED, PANIC = range(7)
OW_STANDARD_FONT_HELVETICA = 0

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
declare("ow_pdf_document_save_path", c_int, c_void_p, c_char_p)
declare("ow_pdf_document_save_bytes", c_int, c_void_p, POINTER(POINTER(c_uint8)), POINTER(c_size_t))
declare("ow_pdf_document_free", None, c_void_p)
declare("ow_pdf_font_new_standard", c_int, ctypes.c_int32, POINTER(c_void_p))
declare("ow_pdf_font_text_width", c_int, c_void_p, c_char_p, c_double, POINTER(c_double))
declare("ow_pdf_font_free", None, c_void_p)


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
        pdf = self.save_bytes(doc)
        self.assertIn(b"/Count 2", pdf)
        self.assertIn(b"(page two) Tj", pdf)

        width = c_double()
        self.check(lib.ow_pdf_font_text_width(font, b"H", 10.0, byref(width)))
        self.assertAlmostEqual(width.value, 7.22)

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


if __name__ == "__main__":
    unittest.main()
