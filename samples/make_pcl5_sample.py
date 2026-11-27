#!/usr/bin/env python3
"""Write samples/feature-test.pcl (PCL 5 + HP-GL/2) and samples/feature-test.plt."""
import struct, zlib
E = b'\x1b'
out = bytearray()
def w(*xs):
    for x in xs:
        out.extend(x if isinstance(x, (bytes, bytearray)) else x.encode('latin-1'))

def pe_num(v):
    n = (abs(v) << 1) | (1 if v < 0 else 0)
    s = bytearray()
    while n >= 64:
        s.append(63 + (n % 64)); n //= 64
    s.append(191 + n)
    return bytes(s)

def pe(points, pen_up_first=True):
    """relative polyline-encoded"""
    b = bytearray(b'PE')
    first = True
    for dx, dy in points:
        if first and pen_up_first: b += b'<'
        b += pe_num(dx) + pe_num(dy)
        first = False
    return bytes(b) + b';'

# ---------------- page 1: Letter portrait
w('\x1b%-12345X@PJL JOB NAME="PCL5 feature test"\r\n@PJL SET COPIES=1\r\n@PJL ENTER LANGUAGE=PCL\r\n')
w(E, 'E', E, '&l2A', E, '&l0O', E, '&l0E', E, '&u600D')   # letter, portrait, top margin 0, 600 units
w(E, '*p150x200Y')
w(E, '(19U', E, '(s1p24v0s3b16602T', 'PCL 5 feature test')
w(E, '*p150x320Y', E, '(8U', E, '(s0p10h12v0s0b4099T', 'Courier 10 pitch: 0123456789 ABC')
w(E, '*p150x420Y', E, '(19U', E, '(s1p14v1s0b16901T', 'Times italic ', E, '(s3B', 'bold italic ', E, '(s0B', 'caf\xe9 na\xefve')
w(E, '*p150x520Y', E, '(s1p12v0s0b16602T', E, '&dD', 'Underlined text', E, '&d@', ' then normal')
# rectangles: solid, shaded 25%, white over solid
w(E, '*p150x620Y', E, '*c600a300b0P')
w(E, '*p900x620Y', E, '*c600a300b25g2P')
w(E, '*p1650x620Y', E, '*c600a300b0P', E, '*p1750x700Y', E, '*c400a140b1P')
# colour: CID direct by pixel 24-bit, raster gradient 120x60 at 150 dpi, packbits
w(E, '*v6W', bytes([0, 3, 8, 8, 8, 8]))
w(E, '*p150x1000Y', E, '*t150R', E, '*r120s60T', E, '*b2M', E, '*r1A')
for y in range(60):
    row = bytearray()
    for x in range(120):
        row += bytes([int(255 * x / 119), int(255 * y / 59), 128])
    # packbits literal chunks
    pk = bytearray(); i = 0
    while i < len(row):
        n = min(128, len(row) - i); pk.append(n - 1); pk += row[i:i+n]; i += n
    w(E, '*b%dW' % len(pk), pk)
w(E, '*rC')
# foreground colour text: palette index from CID default palette (1 = red)
w(E, '*v255a0b0c5I', E, '*v5S', E, '*p1100x1100Y', E, '(s1p18v0s3b16602T', 'Red text via palette')
w(E, '*v0S')
# mono raster, delta row compression, 300 dpi checkerboard 64x64
w(E, '*r1U', E, '*p150x1300Y', E, '*t300R', E, '*r64S', E, '*b3M', E, '*r1A')
prev = bytes(8)
for y in range(64):
    row = bytes([0xF0 if (y // 4) % 2 == 0 else 0x0F] * 8)
    # delta row: replace all 8 bytes at offset 0 -> command (7<<5)|0
    w(E, '*b9W', bytes([(7 << 5) | 0]) + row)
w(E, '*rB')
# HP-GL/2: picture frame = 4in x 3in anchored at cursor
w(E, '*p900x1300Y', E, '*c2880x2160Y', E, '*c0T', E, '%1B')
w('IN;SP1;PW0.5;PA0,0;PU0,0;PD4064,0,4064,3048,0,3048,0,0;')         # frame border
w('SP2;PW0.8;PU500,500;PD1500,2500,2500,500;')                       # red V
w('SP5;PU3000,1500;CI600;')                                           # blue circle
w('SP3;FT10,50;PU500,2600;RA1500,2900;')                              # green 50% rect
w('SP1;PU1700,2700;', pe([(0, 0), (300, 200), (300, -200), (300, 200), (300, -200)]))  # zig-zag via PE
w('PM0;PU2600,2200;PD3400,2200,3000,2900;PM2;SP6;FP;SP1;EP;')         # filled triangle
w('SP1;SI0.3,0.4;PU300,150;LBHP-GL/2 label\x03;')
w(E, '%0A')
# macro used as overlay on every page: a footer line
w(E, '&f1y0X', E, '*p150x6000Y', E, '(s1p10v0s0b16602T', 'Overlay macro footer', E, '&f1X')
w(E, '&f1y4X')
w('\x0c')
# ---------------- page 2: A4 landscape, soft bitmap font
w(E, '&l26A', E, '&l1O', E, '&l0E')
# bitmap soft font id 5: header (format 0) + 2 chars 'A','B'
hdr = bytearray(64)
struct.pack_into('>H', hdr, 0, 64); hdr[2] = 0; hdr[3] = 0
struct.pack_into('>H', hdr, 6, 40); struct.pack_into('>H', hdr, 8, 40); struct.pack_into('>H', hdr, 10, 50)
hdr[13] = 1; struct.pack_into('>H', hdr, 14, 277); struct.pack_into('>H', hdr, 16, 160); struct.pack_into('>H', hdr, 18, 200)
hdr[48:48+9] = b'TestBitmp'
w(E, '*c5D', E, ')s64W', hdr)
def bchar(code, rows):
    h = len(rows); wd = len(rows[0])
    bits = bytearray()
    for r in rows:
        v = 0
        for i, c in enumerate(r):
            if c == '#': v |= 1 << (7 - i)
        bits.append(v)
    desc = bytes([4, 0, 14, 1, 0, 0]) + struct.pack('>hhhhh', 0, h, wd, h, 9 * 4)
    w(E, '*c%dE' % code, E, '(s%dW' % (len(desc) + len(bits)), desc, bits)
A = ['..##....', '.#..#...', '#....#..', '######..', '#....#..', '#....#..', '#....#..', '........']
B = ['#####...', '#....#..', '#####...', '#....#..', '#....#..', '#....#..', '#####...', '........']
bchar(65, A); bchar(66, B)
w(E, '*p150x150Y', E, '(s1p24v0s3b16602T', 'Page 2: A4 landscape')
w(E, '*p150x300Y', E, '(5X', 'ABBA', E, '(3@')
w(E, '*p150x420Y', E, '(s1p12v0s0b16602T', 'Soft bitmap font above (ID 5)')
w(E, '%0B', 'IN;SP1;PW1;PU1000,1000;PD9000,1000;LT2;PU1000,1300;PD9000,1300;LT;', E, '%0A')
w('\x0c', E, 'E')
w('\x1b%-12345X@PJL EOJ\r\n\x1b%-12345X')
open('samples/feature-test.pcl', 'wb').write(out)

# stand-alone HP-GL/2 plot
plt = b'IN;PS10160,7620;SP1;PW0.4;PU0,0;PD10160,0,10160,7620,0,7620,0,0;SP2;PU1000,1000;CI800;SP5;PU3000,1000;PD5000,3000,7000,1000;SP1;SI0.5,0.7;PU1000,6500;LBStand-alone HP-GL/2 plot\x03;SP3;FT1;PU7500,5000;RA9500,7000;'
open('samples/feature-test.plt', 'wb').write(plt)
print(len(out), 'bytes')
