"""Checks the PDF that officewright-pdf's example `links` writes: links to a URI and to
named destinations on three pages, and 153 named destinations, more than one node of the
name tree holds. With tools that share no code with it:

- qpdf validates the file structure, and its JSON gives each page's link annotations and
  the name tree, which this test walks to find the page of each link's destination;
- pdfinfo lists the named destinations with their pages and positions.

OW_LINKS must point at the example's binary (scripts/check.sh sets it).
"""

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_embedded_fonts import run

# Each named destination the example adds: its page, from 1, and its point.
DESTINATIONS = {
    "Start": (1, 72, 800),
    **{f"d{i:03}": (2, 36, 800 - 5 * i) for i in range(150)},
    "Appendix": (3, 72, 770.25),
    "Глава": (3, 0, 400),
}

# Each page's links, in the order they were added: the rectangle and where they go.
LINKS = [
    [
        ([72, 700, 192, 714.5], ("uri", "https://example.com/a%20b/(%D0%96)")),
        ([72, 680, 152, 694], ("dest", "Appendix")),
    ],
    [],
    [
        ([300, 400, 400, 420], ("dest", "Глава")),
        ([300, 300, 400, 320], ("dest", "Start")),
    ],
]


def text(value):
    """A string of qpdf's JSON: "u:" before text, "b:" before bytes in hexadecimal."""
    kind, _, rest = value.partition(":")
    if kind == "u":
        return rest
    raise AssertionError(f"not a text string: {value!r}")


class LinksTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.pdf = Path(cls.directory.name) / "links.pdf"
        run([os.environ["OW_LINKS"], str(cls.pdf)])
        json_text = run(["qpdf", "--json=2", "--json-key=pages", "--json-key=qpdf", str(cls.pdf)]).stdout
        document = json.loads(json_text)
        cls.objects = document["qpdf"][1]
        cls.pages = [page["object"] for page in document["pages"]]

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def resolve(self, value):
        """The value of an indirect reference such as "4 0 R", or `value` itself."""
        if isinstance(value, str) and value.endswith(" R"):
            return self.objects[f"obj:{value}"]["value"]
        return value

    def test_qpdf_finds_no_errors_or_warnings(self):
        result = subprocess.run(["qpdf", "--check", str(self.pdf)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("WARNING", result.stdout + result.stderr)

    def test_each_page_has_its_links(self):
        self.assertEqual(len(self.pages), 3)
        for number, (page, expected) in enumerate(zip(self.pages, LINKS), 1):
            annotations = self.resolve(page).get("/Annots", [])
            links = []
            for annotation in annotations:
                # Each annotation is an object of its own.
                self.assertRegex(annotation, r"^\d+ 0 R$")
                link = self.resolve(annotation)
                self.assertEqual((link["/Type"], link["/Subtype"]), ("/Annot", "/Link"))
                # No border: the link shows as what the page draws.
                self.assertEqual(link["/Border"], [0, 0, 0])
                if "/A" in link:
                    self.assertNotIn("/Dest", link)
                    self.assertEqual(link["/A"]["/S"], "/URI")
                    target = ("uri", text(link["/A"]["/URI"]))
                else:
                    target = ("dest", text(link["/Dest"]))
                links.append((link["/Rect"], target))
            self.assertEqual(links, expected, f"page {number}")

    def names(self, node, depth=0):
        """The names under a node of the name tree and the page each goes to, in order,
        checking each node's limits on the way."""
        node = self.resolve(node)
        if "/Names" in node:
            self.assertNotIn("/Kids", node)
            flat = node["/Names"]
            entries = [(text(key), self.resolve(value)) for key, value in zip(flat[::2], flat[1::2])]
            for name, destination in entries:
                page, kind, _, _, zoom = destination
                self.assertEqual((kind, zoom), ("/XYZ", None), name)
        else:
            entries = [entry for kid in node["/Kids"] for entry in self.names(kid, depth + 1)]
        if depth == 0:
            # The root has no limits.
            self.assertNotIn("/Limits", node)
        else:
            self.assertEqual([text(limit) for limit in node["/Limits"]], [entries[0][0], entries[-1][0]])
        return entries

    def test_the_name_tree_finds_each_destination(self):
        catalog = self.objects["obj:1 0 R"]["value"]
        root = self.resolve(catalog["/Names"]["/Dests"])
        # More names than one node holds.
        self.assertIn("/Kids", root)
        entries = self.names(catalog["/Names"]["/Dests"])
        names = [name for name, _ in entries]
        keys = [name.encode("ascii") if name.isascii() else b"\xfe\xff" + name.encode("utf-16-be") for name in names]
        # Sorted by their bytes as the file stores them.
        self.assertEqual(keys, sorted(keys))
        found = {name: (self.pages.index(page) + 1, x, y) for name, (page, _, x, y, _) in entries}
        self.assertEqual(found, DESTINATIONS)
        # Every link to a destination finds it.
        for links in LINKS:
            for _, (kind, name) in links:
                if kind == "dest":
                    self.assertIn(name, found)

    def test_pdfinfo_lists_every_destination(self):
        lines = run(["pdfinfo", "-dests", str(self.pdf)]).stdout.decode("utf-8").splitlines()
        self.assertEqual(lines[0].split(), ["Page", "Destination", "Name"])
        listed = {}
        for line in lines[1:]:
            match = re.fullmatch(r'\s*(\d+) \[ XYZ\s+(\S+)\s+(\S+)\s+null\s*\] "(.*)"', line)
            self.assertIsNotNone(match, line)
            page, x, y, name = match.groups()
            listed[name] = (int(page), float(x), float(y))
        self.assertEqual(listed.keys(), DESTINATIONS.keys())
        for name, (page, x, y) in DESTINATIONS.items():
            # pdfinfo prints whole points.
            self.assertEqual(listed[name][0], page, name)
            self.assertLessEqual(abs(listed[name][1] - x), 1, name)
            self.assertLessEqual(abs(listed[name][2] - y), 1, name)


if __name__ == "__main__":
    unittest.main()
