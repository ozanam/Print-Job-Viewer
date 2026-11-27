"""Generate a PCL XL (PCL 6) test job exercising many features."""
import struct, sys, math

out = bytearray()
def b(*xs): out.extend(bytes(xs))
def ubyte(v): b(0xC0, v)
def uint16(v): out.extend(b'\xC1' + struct.pack('<H', v))
def sint16(v): out.extend(b'\xC3' + struct.pack('<h', v))
def real32(v): out.extend(b'\xC5' + struct.pack('<f', v))
def ubyte_arr(bs): out.extend(b'\xC8\xC1' + struct.pack('<H', len(bs)) + bytes(bs))
def uint16_arr(vs): out.extend(b'\xC9\xC1' + struct.pack('<H', len(vs)) + b''.join(struct.pack('<H', v) for v in vs))
def sint16_arr(vs): out.extend(b'\xCB\xC1' + struct.pack('<H', len(vs)) + b''.join(struct.pack('<h', v) for v in vs))
def uint16_xy(x, y): out.extend(b'\xD1' + struct.pack('<HH', x, y))
def sint16_xy(x, y): out.extend(b'\xD3' + struct.pack('<hh', x, y))
def real32_xy(x, y): out.extend(b'\xD5' + struct.pack('<ff', x, y))
def uint16_box(a, c, d, e): out.extend(b'\xE1' + struct.pack('<HHHH', a, c, d, e))
def attr(i): b(0xF8, i)
def op(o): b(o)
def data(bs):
    if len(bs) < 256: out.extend(bytes([0xFB, len(bs)]) + bytes(bs))
    else: out.extend(b'\xFA' + struct.pack('<I', len(bs)) + bytes(bs))

UEL = b'\x1b%-12345X'
out += UEL + b'@PJL JOB NAME="XL feature test"\r\n@PJL SET USERNAME="tester"\r\n@PJL SET COPIES=2\r\n@PJL SET DUPLEX=ON\r\n@PJL SET BINDING=LONGEDGE\r\n@PJL SET PAPER=A4\r\n@PJL SET RESOLUTION=600\r\n@PJL COMMENT "made by mkxl.py"\r\n@PJL ENTER LANGUAGE=PCLXL\r\n'
out += b') HP-PCL XL;2;0;Comment Test generator\r\n'

uint16_xy(600, 600); attr(137); ubyte(0); attr(134); ubyte(0); attr(143); op(0x41)  # BeginSession
ubyte(0); attr(136); ubyte(1); attr(130); op(0x48)  # OpenDataSource

# ---- downloaded TrueType font from Arial ----
def load_ttf(path):
    d = open(path, 'rb').read()
    n = struct.unpack('>H', d[4:6])[0]
    t = {}
    for i in range(n):
        tag, cs, off, ln = struct.unpack('>4sIII', d[12+16*i:28+16*i])
        t[tag.decode()] = d[off:off+ln]
    return t
fontpath = None
for p in ['/System/Library/Fonts/Supplemental/Arial.ttf', '/Library/Fonts/Arial.ttf']:
    try:
        open(p,'rb'); fontpath = p; break
    except Exception: pass
tt = load_ttf(fontpath)
head = tt['head']; loca_long = struct.unpack('>h', head[50:52])[0] != 0
numg = struct.unpack('>H', tt['maxp'][4:6])[0]
loca = tt['loca']
def glyph(gid):
    if loca_long: a, c = struct.unpack('>II', loca[gid*4:gid*4+8])
    else:
        a, c = struct.unpack('>HH', loca[gid*2:gid*2+4]); a*=2; c*=2
    return tt['glyf'][a:c]
# cmap format 4 lookup
def cmap_lookup():
    cm = tt['cmap']; n = struct.unpack('>H', cm[2:4])[0]
    for i in range(n):
        pid, eid, off = struct.unpack('>HHI', cm[4+8*i:12+8*i])
        if pid == 3 and eid == 1:
            sub = cm[off:]
            segx2 = struct.unpack('>H', sub[6:8])[0]; seg = segx2//2
            ends = struct.unpack('>%dH'%seg, sub[14:14+segx2])
            starts = struct.unpack('>%dH'%seg, sub[16+segx2:16+2*segx2])
            deltas = struct.unpack('>%dh'%seg, sub[16+2*segx2:16+3*segx2])
            roff_pos = 16+3*segx2
            roffs = struct.unpack('>%dH'%seg, sub[roff_pos:roff_pos+segx2])
            def f(c):
                for k in range(seg):
                    if starts[k] <= c <= ends[k]:
                        if roffs[k] == 0: return (c + deltas[k]) & 0xFFFF
                        p = roff_pos + 2*k + roffs[k] + 2*(c-starts[k])
                        g = struct.unpack('>H', sub[p:p+2])[0]
                        return (g + deltas[k]) & 0xFFFF if g else 0
                return 0
            return f
cm = cmap_lookup()
# GT segment: a small sfnt with head, hhea, hmtx, maxp
tables = [('head', head), ('hhea', tt['hhea']), ('hmtx', tt['hmtx']), ('maxp', tt['maxp'])]
nt = len(tables)
sf = struct.pack('>IHHHH', 0x10000, nt, 64, 2, 0)
off = 12 + 16*nt; dirs = b''; body = b''
for tag, tb in tables:
    dirs += struct.pack('>4sIII', tag.encode(), 0, off + len(body), len(tb))
    body += tb + b'\0' * ((4 - len(tb) % 4) % 4)
gt = sf + dirs + body
hdr = struct.pack('>BBHBBH', 0, 0, 629, 0, 0, 256)  # format, orient, symset 19U, TrueType, variety, numchars
hdr += b'PA' + struct.pack('>I', 10) + b'\0' * 10
hdr += b'GT' + struct.pack('>I', len(gt)) + gt
hdr += b'\xff\xff' + struct.pack('>I', 0)
fname = b'ArialDL         '[:16]
ubyte_arr(fname); attr(168); ubyte(0); attr(169); uint16(len(hdr)); attr(167); op(0x4F)
for i in range(0, len(hdr), 16000):
    chunk = hdr[i:i+16000]
    uint16(len(chunk)); attr(163); op(0x50); data(chunk)
op(0x51)
ubyte_arr(fname); attr(168); op(0x52)
text_dl = "Downloaded TrueType: Grüße, naïve café – 123"
needed = set()
for ch in text_dl:
    needed.add(ord(ch))
gids_needed = {}
for c in needed:
    g = cm(c); gids_needed[c] = g
def add_char(code, gid):
    gd = glyph(gid)
    body = struct.pack('>H', gid) + gd
    cd = struct.pack('>BBH', 1, 1, len(body)) + body
    uint16(code); attr(162); uint16(len(cd)); attr(163); op(0x53); data(cd)
# composite component glyphs: download them under private codes so the viewer can find them
comp_extra = set()
for c, g in gids_needed.items():
    gd = glyph(g)
    if len(gd) > 10 and struct.unpack('>h', gd[0:2])[0] < 0:
        p = 10
        while True:
            fl, cg = struct.unpack('>HH', gd[p:p+4]); comp_extra.add(cg)
            p += 4 + (4 if fl & 1 else 2)
            if fl & 8: p += 2
            elif fl & 0x40: p += 4
            elif fl & 0x80: p += 8
            if not fl & 0x20: break
cp1252 = {0x2013: 0x96}
for c, g in gids_needed.items():
    code = cp1252.get(c, c)
    add_char(code, g)
for k, g in enumerate(sorted(comp_extra)):
    add_char(0xF000 + k, g)
op(0x54)

# ---- bitmap font with 2 chars ----
bhdr = struct.pack('>BBHBBH', 0, 0, 629, 254, 0, 2)
bhdr += b'BR' + struct.pack('>H', 4) + struct.pack('>HH', 300, 300)
bhdr += b'\xff\xff' + struct.pack('>H', 0)
bname = b'BitmapFont      '
ubyte_arr(bname); attr(168); ubyte(0); attr(169); uint16(len(bhdr)); attr(167); op(0x4F)
uint16(len(bhdr)); attr(163); op(0x50); data(bhdr); op(0x51)
ubyte_arr(bname); attr(168); op(0x52)
def bitmap_char(code, rows):
    w = len(rows[0]); h = len(rows)
    bits = b''
    for r in rows:
        v = 0; by = b''
        for x, ch in enumerate(r):
            if ch == '#': v |= 0x80 >> (x % 8)
            if x % 8 == 7 or x == w-1: by += bytes([v]); v = 0
        bits += by
    cd = struct.pack('>BBhhHH', 0, 0, 0, h, w, h) + bits
    uint16(code); attr(162); uint16(len(cd)); attr(163); op(0x53); data(cd)
smile = ["....######....", "..##......##..", ".#..........#.", "#...##..##...#", "#...##..##...#", "#............#", "#..#......#..#", "#...######...#", ".#..........#.", "..##......##..", "....######...."]
bitmap_char(65, smile)
bitmap_char(66, ["##########"] * 3 + ["##......##"] * 4 + ["##########"] * 3)
op(0x54)

def begin_page(orient, size):
    ubyte(orient); attr(40); ubyte(size); attr(37); ubyte(1); attr(38); ubyte(1); attr(53); ubyte(0); attr(54); op(0x43)

def set_brush_rgb(r, g, bb): ubyte_arr([r, g, bb]); attr(11); op(0x63)
def set_brush_gray(g): ubyte(g); attr(9); op(0x63)
def set_pen_rgb(r, g, bb): ubyte_arr([r, g, bb]); attr(11); op(0x79)
def null_pen(): ubyte(0); attr(5); op(0x79)
def null_brush(): ubyte(0); attr(4); op(0x63)
def pen_width(w): uint16(w); attr(75); op(0x7A)
def set_font(name, size, symset=629):
    ubyte_arr(name); attr(168); real32(size); attr(166); uint16(symset); attr(170); op(0x6F)
def cursor(x, y): sint16_xy(x, y); attr(76); op(0x6B)
def text(s, spacing=None):
    codes = [cp1252.get(ord(c), ord(c)) for c in s]
    ubyte_arr(codes); attr(171)
    if spacing is not None:
        sint16_arr(spacing); attr(175)
    op(0xA8)

# ========== Page 1: A4 portrait ==========
ubyte(2); attr(3); op(0x6A)  # RGB colour space
begin_page(0, 2)
op(0x61)
# title text with resident font
set_brush_rgb(0, 0, 0)
set_font(b'Arial           ', 300.0)
cursor(300, 500)
text("PCL XL feature test")
set_font(b'TimesNewRmn     ', 150.0)
cursor(300, 750); text("Resident serif font (Times New Roman substitute)")
set_font(b'CourierNew      ', 150.0)
cursor(300, 950); text("Resident monospace: 0123456789 ABC xyz")
# downloaded TT font text
set_brush_rgb(0, 0, 160)
set_font(fname, 180.0)
cursor(300, 1200); text(text_dl)
# bitmap font
set_brush_rgb(200, 0, 0)
set_font(bname, 100.0)
cursor(300, 1400); text("ABAB", [30, 30, 30, 30])
# rectangles
null_pen(); set_brush_rgb(255, 200, 0)
uint16_box(300, 1600, 1500, 2400); attr(66); op(0xA0)
set_pen_rgb(0, 128, 0); pen_width(20); null_brush()
uint16_box(1700, 1600, 2900, 2400); attr(66); op(0x98)  # ellipse
# rounded rect, filled + stroked
set_brush_rgb(100, 180, 255); set_pen_rgb(0, 0, 0); pen_width(8)
uint16_box(3100, 1600, 4600, 2400); attr(66); uint16_xy(300, 300); attr(68); op(0xA2)
# lines path with embedded points (LineRelPath? use LinePath abs with endpoints)
op(0x85); cursor(300, 2700)
for (x, y) in [(900, 3200), (1500, 2700), (2100, 3200), (2700, 2700)]:
    uint16_xy(x, y); attr(69); op(0x9B)
null_brush(); set_pen_rgb(160, 0, 160); pen_width(30)
ubyte(1); attr(71); op(0x71)  # round cap
op(0x86)
# bezier
op(0x85); cursor(3000, 3200)
uint16_xy(3300, 2500); attr(81); uint16_xy(4200, 3800); attr(82); uint16_xy(4600, 2700); attr(69); op(0x93)
set_pen_rgb(255, 0, 0); pen_width(15); op(0x86)
# pie
set_brush_rgb(0, 200, 120); null_pen()
uint16_box(300, 3500, 1300, 4500); attr(66); uint16_xy(1300, 4000); attr(79); uint16_xy(800, 3500); attr(69); op(0x9E)
# dashed line
op(0x85); cursor(1600, 4000); uint16_xy(4600, 4000); attr(69); op(0x9B)
uint16_arr([60, 30]); attr(74); op(0x70)
set_pen_rgb(0, 0, 0); pen_width(10); null_brush(); op(0x86)
ubyte(0); attr(78); op(0x70)
# gray gradient image, 8-bit gray, RLE compressed
ubyte(1); attr(3); op(0x6A)  # gray space
W, H = 256, 64
cursor(300, 4700)
ubyte(0); attr(100); ubyte(2); attr(98); uint16(W); attr(108); uint16(H); attr(107); uint16_xy(2400, 600); attr(103); op(0xB0)
rows = b''
for y in range(H):
    rows += bytes(range(256))
def packbits(dat):
    o = bytearray(); i = 0
    while i < len(dat):
        j = i
        while j < len(dat) and j - i < 127 and dat[j] == dat[i]: j += 1
        if j - i >= 2:
            o += bytes([(257 - (j - i)) & 0xFF, dat[i]]); i = j
        else:
            k = i
            while k < len(dat) and k - i < 128 and not (k + 1 < len(dat) and dat[k] == dat[k+1]): k += 1
            if k == i: k = i + 1
            o += bytes([k - i - 1]) + dat[i:k]; i = k
    return bytes(o)
uint16(0); attr(109); uint16(H); attr(99); ubyte(1); attr(101); op(0xB1); data(packbits(rows))
op(0xB2)
# RGB image uncompressed (8 bit, 3 comps) with rows padded to 4 bytes
ubyte(2); attr(3); op(0x6A)
W, H = 50, 40
cursor(2900, 4700)
ubyte(0); attr(100); ubyte(2); attr(98); uint16(W); attr(108); uint16(H); attr(107); uint16_xy(1500, 1200); attr(103); op(0xB0)
rowb = W * 3; pad = (4 - rowb % 4) % 4
img = b''
for y in range(H):
    for x in range(W):
        img += bytes([x * 5, y * 6, 128])
    img += b'\0' * pad
uint16(0); attr(109); uint16(H); attr(99); ubyte(0); attr(101); op(0xB1); data(img)
op(0xB2)
# raster pattern brush: 8x8 checker 1-bit indexed with palette
ubyte(2); attr(3); ubyte(0); attr(2); ubyte_arr([255, 255, 255, 220, 0, 0]); attr(6); op(0x6A)
ubyte(1); attr(100); ubyte(0); attr(98); uint16(8); attr(108); uint16(8); attr(107); uint16_xy(80, 80); attr(103); sint16(7); attr(105); ubyte(2); attr(104); op(0xB3)
pat = b''.join(bytes([0xF0 if (y // 4) % 2 == 0 else 0x0F, 0, 0, 0]) for y in range(8))
uint16(0); attr(109); uint16(8); attr(99); ubyte(0); attr(101); op(0xB4); data(pat)
op(0xB5)
ubyte(2); attr(3); op(0x6A)
sint16(7); attr(8); op(0x63)
set_pen_rgb(0, 0, 0); pen_width(6)
uint16_box(300, 5600, 2500, 6600); attr(66); op(0xA0)
# clip test: clip to ellipse then fill big rect
op(0x61)
op(0x85); uint16_box(2900, 5600, 4600, 6600); attr(66); op(0x99)
ubyte(0); attr(83); op(0x62)
set_brush_rgb(255, 120, 0); null_pen()
for k in range(0, 1800, 120):
    uint16_box(2900 + k, 5600, 2960 + k, 6600); attr(66); op(0xA0)
op(0x60)
# rotated text
op(0x61)
sint16_xy(4500, 6900); attr(42); op(0x75)
real32(-30.0); attr(41); op(0x76)
set_brush_rgb(0, 100, 0); set_font(b'Arial           ', 200.0)
cursor(0, 0); text("Rotated text")
op(0x60)
op(0x60)
uint16(1); attr(49); op(0x44)  # EndPage copies

# ========== Page 2: Letter landscape ==========
begin_page(1, 0)
set_brush_rgb(0, 0, 0)
set_font(b'Arial           ', 400.0)
cursor(600, 800); text("Page 2 - Letter landscape")
set_font(b'Helvetica       ', 200.0)
cursor(600, 1300); text("Text with explicit spacing:", None)
set_font(b'CourierNew      ', 200.0)
cursor(600, 1700); text("SPACED", [200, 200, 200, 200, 200, 200])
null_pen(); set_brush_gray(128)
uint16_box(600, 2200, 5800, 2400); attr(66); op(0xA0)
uint16(1); attr(49); op(0x44)

op(0x49)  # CloseDataSource
op(0x42)  # EndSession
out += UEL + b'@PJL EOJ NAME="XL feature test"\r\n' + UEL
open(sys.argv[1], 'wb').write(out)
print(len(out), 'bytes')
