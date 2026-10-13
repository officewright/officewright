"""Small PNG and JPEG files made from scratch, for tests/pdf-output/test_images.py and as
seeds of the fuzz targets png_decode and jpeg_header.

    python3 tests/pdf-output/images.py fuzz/seeds

writes the seeds to fuzz/seeds/png and fuzz/seeds/jpeg; test_images.py checks that the
committed seeds are what this script writes.
"""

import struct
import sys
import zlib
from pathlib import Path


def zlib_stored(data):
    """`data` as a zlib stream of stored deflate blocks (RFC 1950 and 1951, 3.2.4): the same
    bytes with every zlib library, unlike compressed output, which differs between zlib and
    zlib-ng."""
    out = b"\x78\x01"
    blocks = [data[i : i + 65535] for i in range(0, len(data), 65535)] or [b""]
    for i, block in enumerate(blocks):
        out += bytes([1 if i == len(blocks) - 1 else 0]) + struct.pack("<HH", len(block), len(block) ^ 0xFFFF)
        out += block
    return out + struct.pack(">I", zlib.adler32(data))


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def _filter(kind, row, previous, bpp):
    """`row` filtered with filter type `kind` (PNG specification, 7.3 and 9.2)."""
    out = bytearray()
    for i, x in enumerate(row):
        a = row[i - bpp] if i >= bpp else 0
        b = previous[i] if previous else 0
        c = previous[i - bpp] if previous and i >= bpp else 0
        predictor = [0, a, b, (a + b) // 2, _paeth(a, b, c)][kind]
        out.append((x - predictor) & 0xFF)
    return bytes(out)


def png(width, height, color_type, bits, rows, palette=None, transparency=None, interlaced=False, filters=(0,)):
    """A PNG file. `rows` gives the samples of each row, already at the bit depth, as a
    list of lists of pixels, each a tuple of samples. The rows use the filter types of
    `filters` in turn."""

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color_type]

    def pack(pixels):
        samples = [s for pixel in pixels for s in pixel]
        if bits == 16:
            return b"".join(struct.pack(">H", s) for s in samples)
        if bits == 8:
            return bytes(samples)
        out = bytearray((len(samples) * bits + 7) // 8)
        for i, s in enumerate(samples):
            bit = i * bits
            out[bit // 8] |= s << (8 - bits - bit % 8)
        return bytes(out)

    if interlaced:
        passes = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    else:
        passes = [(0, 0, 1, 1)]
    bpp = max(1, channels * bits // 8)
    data = b""
    count = 0
    for x0, y0, dx, dy in passes:
        xs = range(x0, width, dx)
        if not xs:
            continue
        previous = None
        for y in range(y0, height, dy):
            row = pack([rows[y][x] for x in xs])
            kind = filters[count % len(filters)]
            count += 1
            data += bytes([kind]) + _filter(kind, row, previous, bpp)
            previous = row
    assert channels == len(rows[0][0])
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, bits, color_type, 0, 0, int(interlaced)))
    if palette is not None:
        out += chunk(b"PLTE", bytes(c for entry in palette for c in entry))
    if transparency is not None:
        out += chunk(b"tRNS", transparency)
    out += chunk(b"IDAT", zlib_stored(data))
    out += chunk(b"IEND", b"")
    return out


def _bits(value, size):
    return format(value, f"0{size}b") if size else ""


def jpeg(width, height, blocks, adobe=None):
    """A baseline JPEG file whose 8 × 8 blocks are each of one colour: `blocks` gives, row
    by row, the samples of each block, one per component, 0 to 255. With `adobe`, an APP14
    segment with that colour transform (0 for RGB or CMYK, 1 for YCbCr), whose CMYK
    samples are inverted as Adobe's applications write them."""
    components = len(blocks[0][0])
    out = b"\xff\xd8"
    if adobe is None:
        out += b"\xff\xe0\x00\x10JFIF\x00\x01\x02\x00\x00\x01\x00\x01\x00\x00"
    else:
        out += b"\xff\xee\x00\x0eAdobe\x00\x64\x00\x00\x00\x00" + bytes([adobe])
    # Quantization table 0: every coefficient divided by 1.
    out += b"\xff\xdb\x00\x43\x00" + b"\x01" * 64
    out += b"\xff\xc0" + struct.pack(">HBHHB", 8 + 3 * components, 8, height, width, components)
    for c in range(components):
        out += bytes([c + 1, 0x11, 0])
    # DC table 0: categories 0 to 11, each with a 4-bit code; AC table 0: only the end of
    # block, coded 0.
    dc = bytes([0, 0, 0, 12] + [0] * 12) + bytes(range(12))
    ac = bytes([1] + [0] * 15) + b"\x00"
    out += b"\xff\xc4" + struct.pack(">H", 3 + len(dc)) + b"\x00" + dc
    out += b"\xff\xc4" + struct.pack(">H", 3 + len(ac)) + b"\x10" + ac
    out += b"\xff\xda" + struct.pack(">HB", 6 + 2 * components, components)
    for c in range(components):
        out += bytes([c + 1, 0x00])
    out += b"\x00\x3f\x00"
    bits = ""
    previous = [0] * components
    for row in blocks:
        for block in row:
            for c, sample in enumerate(block):
                # The DC coefficient of a block of one value: 8 × (value − 128).
                dc_value = 8 * (sample - 128)
                diff = dc_value - previous[c]
                previous[c] = dc_value
                category = abs(diff).bit_length()
                extra = diff if diff >= 0 else diff + (1 << category) - 1
                bits += _bits(category, 4) + _bits(extra, category) + "0"
    bits += "1" * (-len(bits) % 8)
    scan = bytes(int(bits[i : i + 8], 2) for i in range(0, len(bits), 8))
    out += scan.replace(b"\xff", b"\xff\x00")
    return out + b"\xff\xd9"


def ycbcr(r, g, b):
    """JFIF's conversion of an RGB colour to YCbCr."""
    y = 0.299 * r + 0.587 * g + 0.114 * b
    cb = 128 - 0.168736 * r - 0.331264 * g + 0.5 * b
    cr = 128 + 0.5 * r - 0.418688 * g - 0.081312 * b
    return tuple(max(0, min(255, round(v))) for v in (y, cb, cr))


RED, GREEN, BLUE, WHITE, BLACK = (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255), (0, 0, 0)


def samples():
    """The test images: name, file, and what pdfimages lists for it: (type, width, height,
    colour, components, bits, encoding) for the image and its soft mask, if any."""
    quadrants = [[RED, GREEN], [BLUE, WHITE]]
    # 16 × 15 pixels of many colours, for the filters to work on.
    gradient = [[((x * 16 + y * 7) % 256, (x * y * 5) % 256, (255 - x * 13 - y * 3) % 256) for x in range(16)] for y in range(15)]
    adam7 = [[RED if x < 4 and y < 4 else GREEN if y < 4 else BLUE if x < 4 else WHITE for x in range(8)] for y in range(8)]
    return [
        ("rgb.png", png(2, 2, 2, 8, quadrants), [("image", 2, 2, "rgb", 3, 8, "image")]),
        (
            "palette.png",
            png(2, 1, 3, 4, [[(0,), (1,)]], palette=[RED, BLUE], transparency=b"\xff\x00"),
            [("image", 2, 1, "index", 1, 4, "image")],
        ),
        (
            "rgba.png",
            png(2, 1, 6, 8, [[(0, 0, 255, 255), (255, 0, 0, 0)]]),
            [("image", 2, 1, "rgb", 3, 8, "image"), ("smask", 2, 1, "gray", 1, 8, "image")],
        ),
        ("gray16.png", png(2, 1, 0, 16, [[(0,), (0xFFFF,)]]), [("image", 2, 1, "gray", 1, 16, "image")]),
        ("adam7.png", png(8, 8, 2, 8, adam7, interlaced=True), [("image", 8, 8, "rgb", 3, 8, "image")]),
        (
            "gray-key.png",
            png(2, 1, 0, 8, [[(0x80,), (0x00,)]], transparency=b"\x00\x80"),
            [("image", 2, 1, "gray", 1, 8, "image"), ("smask", 2, 1, "gray", 1, 8, "image")],
        ),
        ("gray.jpg", jpeg(16, 8, [[(0,), (255,)]]), [("image", 16, 8, "gray", 1, 8, "jpeg")]),
        ("ycbcr.jpg", jpeg(16, 8, [[ycbcr(*RED), ycbcr(*BLUE)]]), [("image", 16, 8, "rgb", 3, 8, "jpeg")]),
        # The same pixels with each filter type in turn: passed through, so poppler undoes the
        # filters; and decoded by officewright, forced by a transparent colour that no pixel
        # has. test_images.py compares the two renders.
        (
            "filtered.png",
            png(16, 15, 2, 8, gradient, filters=(0, 1, 2, 3, 4)),
            [("image", 16, 15, "rgb", 3, 8, "image")],
        ),
        (
            "filtered-decoded.png",
            png(16, 15, 2, 8, gradient, transparency=b"\x00\x01\x00\x02\x00\x03", filters=(0, 1, 2, 3, 4)),
            [("image", 16, 15, "rgb", 3, 8, "image")],
        ),
        (
            "cmyk.jpg",
            # Cyan and magenta, inverted.
            jpeg(16, 8, [[(0, 255, 255, 255), (255, 0, 255, 255)]], adobe=0),
            [("image", 16, 8, "cmyk", 4, 8, "jpeg")],
        ),
    ]


def write_seeds(directory):
    """Writes the PNG and JPEG test images as fuzz seeds under `directory`."""
    for name, data, _ in samples():
        kind = "png" if name.endswith(".png") else "jpeg"
        path = Path(directory) / kind / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


if __name__ == "__main__":
    write_seeds(sys.argv[1])
