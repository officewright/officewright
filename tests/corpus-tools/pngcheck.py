"""A strict PNG reader for the corpus tools tests."""

import struct
import zlib


def png_pixels(data):
    """Width, height, pixels per metre and grey values of a PNG, checked chunk by chunk.

    A strict reader of the PNG specification's rules that png_from_pgm relies on: the
    signature, every chunk's length and CRC, IHDR first and IEND last, 8-bit grey without
    interlacing, filter type 0 on every row, and a data stream of exactly the image's size.
    """
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise AssertionError("no PNG signature")
    chunks = []
    position = 8
    while position < len(data):
        (length,) = struct.unpack(">I", data[position : position + 4])
        kind = data[position + 4 : position + 8]
        body = data[position + 8 : position + 8 + length]
        (crc,) = struct.unpack(">I", data[position + 8 + length : position + 12 + length])
        if len(body) != length or crc != zlib.crc32(kind + body) & 0xFFFFFFFF:
            raise AssertionError(f"chunk {kind!r} has a wrong length or CRC")
        chunks.append((kind, body))
        position += 12 + length
    kinds = [kind for kind, _ in chunks]
    if kinds[0] != b"IHDR" or kinds[-1] != b"IEND" or chunks[-1][1]:
        raise AssertionError(f"chunk order {kinds}")
    width, height, depth, colour, compression, filtering, interlace = struct.unpack(
        ">IIBBBBB", chunks[0][1]
    )
    if (depth, colour, compression, filtering, interlace) != (8, 0, 0, 0, 0):
        raise AssertionError("not 8-bit grey without interlacing")
    physical = dict(chunks).get(b"pHYs")
    per_metre = struct.unpack(">IIB", physical) if physical else None
    rows = zlib.decompress(b"".join(body for kind, body in chunks if kind == b"IDAT"))
    stride = width + 1
    if len(rows) != stride * height or any(rows[y * stride] != 0 for y in range(height)):
        raise AssertionError("image data of the wrong size or with filtered rows")
    pixels = b"".join(rows[y * stride + 1 : (y + 1) * stride] for y in range(height))
    return width, height, per_metre, pixels
