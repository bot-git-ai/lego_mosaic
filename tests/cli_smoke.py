#!/usr/bin/env python3
"""External binary regression: standard-library-only 2×2 PNG → exact SVG."""
import binascii
import pathlib
import struct
import subprocess
import tempfile
import zlib

root = pathlib.Path(__file__).resolve().parents[1]

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', binascii.crc32(kind + data) & 0xffffffff)

with tempfile.TemporaryDirectory(prefix='mosaic-cli-') as tmp:
    source = pathlib.Path(tmp) / 'solid.png'
    output = pathlib.Path(tmp) / 'test.svg'
    source.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress((b'\0' + b'\xff\xff\xff' * 2) * 2)) + chunk(b'IEND', b''))
    subprocess.run(['cargo', 'run', '--locked', '--', 'convert', str(source), '--output', str(output)], cwd=root, check=True)
    expected = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 960" width="960" height="960" role="img"><title>Flat exact-color mosaic preview</title><g shape-rendering="crispEdges">'
    for y in range(48):
        for x in range(48):
            expected += f'<rect x="{x*20}" y="{y*20}" width="20" height="20" fill="#f2f3f2"/>'
    expected += '</g></svg>'
    assert output.read_bytes() == expected.encode(), 'CLI SVG differs from exact expected bytes'
    print('PASS external CLI: 2×2 PNG → exact 48×48 SVG bytes')
