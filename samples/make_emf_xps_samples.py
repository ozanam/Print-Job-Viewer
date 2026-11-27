#!/usr/bin/env python3
"""Write samples/feature-test.spl (EMF spool), feature-test.emf and feature-test.xps."""
import struct, zlib, zipfile, uuid, io

def rec(t, payload): 
    return struct.pack('<II', t, 8 + len(payload)) + payload

def emf_page(w_mm=210, h_mm=297, dpi=600):
    pxmm = dpi / 25.4
    devw, devh = int(w_mm * pxmm), int(h_mm * pxmm)
    body = bytearray()
    body += rec(17, struct.pack('<i', 1))                      # SETMAPMODE MM_TEXT
    body += rec(18, struct.pack('<i', 1))                      # transparent bk
    # font: Arial, -200 px (≈24pt at 600dpi)
    lf = struct.pack('<iiiiiBBBBBBBB', -200, 0, 0, 0, 700, 0, 0, 0, 0, 0, 0, 0, 0) + 'Arial'.encode('utf-16le').ljust(64, b'\0')
    body += rec(82, struct.pack('<I', 1) + lf + bytes(320 - 92 + 0)[:0])
    body += rec(37, struct.pack('<I', 1))
    body += rec(24, struct.pack('<I', 0x00000000))
    body += rec(22, struct.pack('<I', 24))                     # TA_BASELINE
    def text(x, y, s):
        n = len(s); so = 76; data = s.encode('utf-16le'); pad = (4 - len(data) % 4) % 4
        dxo = so + len(data) + pad
        payload = struct.pack('<iiii', 0, 0, 0, 0) + struct.pack('<Iff', 1, 1.0, 1.0)
        payload += struct.pack('<iiIII', x, y, n, so, 0) + struct.pack('<iiii', 0, 0, -1, -1) + struct.pack('<I', 0)
        payload += data + bytes(pad)
        return rec(84, payload)
    body += text(300, 500, 'EMF spool feature test')
    lf2 = struct.pack('<iiiiiBBBBBBBB', -100, 0, 0, 0, 400, 1, 1, 0, 0, 0, 0, 0, 0) + 'Times New Roman'.encode('utf-16le').ljust(64, b'\0')
    body += rec(82, struct.pack('<I', 2) + lf2)
    body += rec(37, struct.pack('<I', 2))
    body += rec(24, struct.pack('<I', 0x00C00000))             # blue (BGR)
    body += text(300, 750, 'Italic underlined Times, café')
    # brush + pen, rectangle and ellipse
    body += rec(39, struct.pack('<IIII', 3, 0, 0x0000A0FF, 0))   # orange solid brush
    body += rec(37, struct.pack('<I', 3))
    body += rec(38, struct.pack('<IIiiI', 4, 0, 12, 0, 0x00008000))  # green pen width 12
    body += rec(37, struct.pack('<I', 4))
    body += rec(43, struct.pack('<iiii', 300, 1000, 1500, 1800))
    body += rec(42, struct.pack('<iiii', 1800, 1000, 3200, 1800))
    # polyline16
    pts = [(300, 2200), (900, 2700), (1500, 2200), (2100, 2700), (2700, 2200)]
    body += rec(87, struct.pack('<iiiiI', 0, 0, 0, 0, len(pts)) + b''.join(struct.pack('<hh', *p) for p in pts))
    # StretchDIBits 24-bit gradient 64x32
    bw, bh = 64, 32
    stride = (bw * 3 + 3) & ~3
    bits = bytearray()
    for y in range(bh):
        row = bytearray()
        for x in range(bw):
            row += bytes([200, int(255 * y / (bh - 1)), int(255 * x / (bw - 1))])   # B G R
        bits += row + bytes(stride - len(row))
    bmi = struct.pack('<IiiHHIIiiII', 40, bw, bh, 1, 24, 0, len(bits), 0, 0, 0, 0)
    off_bmi = 80; off_bits = off_bmi + len(bmi)
    payload = struct.pack('<iiii', 0, 0, 0, 0) + struct.pack('<iiiiii', 300, 3000, 0, 0, bw, bh)
    payload += struct.pack('<IIIIIIii', off_bmi, len(bmi), off_bits, len(bits), 0, 0xCC0020, 1200, 600)
    body += rec(81, payload + bmi + bits)
    body += rec(14, struct.pack('<III', 0, 16, 20))
    header_size = 108
    n_records = 0
    total = header_size + len(body)
    hdr = struct.pack('<II', 1, header_size) + struct.pack('<iiii', 0, 0, devw, devh) + struct.pack('<iiii', 0, 0, w_mm * 100, h_mm * 100)
    hdr += b' EMF' + struct.pack('<IIIHHIIIiiii', 0x10000, total, 20, 5, 0, 0, 0, 0, devw, devh, w_mm, h_mm)
    hdr += struct.pack('<III', 0, 0, 0)[:header_size - len(hdr)] if len(hdr) < header_size else b''
    hdr = hdr[:header_size].ljust(header_size, b'\0')
    return hdr + bytes(body)

page = emf_page()
open('samples/feature-test.emf', 'wb').write(page)

# spool file: header, DEVMODE, two pages
name = 'EMF spool test'.encode('utf-16le') + b'\0\0'
hdr = struct.pack('<IIII', 0x10000, 16 + len(name) + (4 - len(name) % 4) % 4, 16, 0) + name
hdr = hdr.ljust(struct.unpack('<I', hdr[4:8])[0], b'\0')
dm = bytearray(220)
dm[0:20] = 'Test Printer'.encode('utf-16le')
struct.pack_into('<HHHH', dm, 64, 0x401, 0, 220, 0)
struct.pack_into('<I', dm, 72, 0x1 | 0x2 | 0x100 | 0x800 | 0x1000)
struct.pack_into('<hhhhhhhhhhhhh', dm, 76, 1, 9, 2970, 2100, 100, 2, 7, 600, 2, 2, 600, 3, 1)
def emri(t, d):
    return struct.pack('<II', t, len(d)) + d
spool = hdr + emri(3, bytes(dm)) + emri(12, page) + emri(12, emf_page())
open('samples/feature-test.spl', 'wb').write(spool)

# ---------------- XPS
font_path = '/System/Library/Fonts/Supplemental/Arial.ttf'
font = bytearray(open(font_path, 'rb').read())
g = uuid.uuid4()
gs = str(g).upper()
key = g.bytes_le  # binary GUID
for i in range(32):
    font[i] ^= key[15 - (i % 16)]

def png_bytes(w, h, rgb):
    raw = b''.join(b'\0' + bytes(rgb[y * w * 3:(y + 1) * w * 3]) for y in range(h))
    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

img = bytearray()
for y in range(48):
    for x in range(96):
        img += bytes([int(255 * x / 95), 80, int(255 * y / 47)])
png = png_bytes(96, 48, img)

fpage = f"""<FixedPage xmlns="http://schemas.microsoft.com/xps/2005/06" Width="816" Height="1056" xml:lang="en-US">
  <Glyphs OriginX="96" OriginY="120" FontRenderingEmSize="32" FontUri="/Resources/{gs}.odttf" Fill="#FF000000" UnicodeString="XPS feature test" />
  <Glyphs OriginX="96" OriginY="170" FontRenderingEmSize="18" FontUri="/Resources/{gs}.odttf" Fill="#FFC00000" UnicodeString="Embedded obfuscated font: caf\u00e9 na\u00efve" />
  <Path Data="M 96,220 L 336,220 L 336,340 L 96,340 Z" Fill="#FF2080FF" Stroke="#FF000000" StrokeThickness="2" />
  <Path Data="F1 M 400,280 A 80,60 0 1 1 560,280 A 80,60 0 1 1 400,280 Z" Fill="#8000A000" />
  <Canvas RenderTransform="1,0,0,1,96,380">
    <Path Data="M 0,0 C 80,-60 160,60 240,0" Stroke="#FFFF6600" StrokeThickness="4" StrokeDashArray="3 2" />
  </Canvas>
  <Path Data="M 96,460 L 384,460 L 384,604 L 96,604 Z">
    <Path.Fill>
      <ImageBrush ImageSource="/Resources/grad.png" Viewbox="0,0,96,48" ViewboxUnits="Absolute" Viewport="96,460,288,144" ViewportUnits="Absolute" />
    </Path.Fill>
  </Path>
</FixedPage>"""
fpage2 = f"""<FixedPage xmlns="http://schemas.microsoft.com/xps/2005/06" Width="1056" Height="816" xml:lang="en-US">
  <Glyphs OriginX="96" OriginY="120" FontRenderingEmSize="28" FontUri="/Resources/{gs}.odttf" Fill="#FF000000" UnicodeString="Page 2 (landscape)" />
</FixedPage>"""
z = io.BytesIO()
with zipfile.ZipFile(z, 'w', zipfile.ZIP_DEFLATED) as zf:
    zf.writestr('[Content_Types].xml', '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="fpage" ContentType="application/vnd.ms-package.xps-fixedpage+xml"/></Types>')
    zf.writestr('_rels/.rels', '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Type="http://schemas.microsoft.com/xps/2005/06/fixedrepresentation" Target="/FixedDocSeq.fdseq" Id="R0"/></Relationships>')
    zf.writestr('FixedDocSeq.fdseq', '<FixedDocumentSequence xmlns="http://schemas.microsoft.com/xps/2005/06"><DocumentReference Source="/Documents/1/FixedDoc.fdoc"/></FixedDocumentSequence>')
    zf.writestr('Documents/1/FixedDoc.fdoc', '<FixedDocument xmlns="http://schemas.microsoft.com/xps/2005/06"><PageContent Source="Pages/1.fpage"/><PageContent Source="Pages/2.fpage"/></FixedDocument>')
    zf.writestr('Documents/1/Pages/1.fpage', fpage)
    zf.writestr('Documents/1/Pages/2.fpage', fpage2)
    zf.writestr(f'Resources/{gs}.odttf', bytes(font))
    zf.writestr('Resources/grad.png', png)
open('samples/feature-test.xps', 'wb').write(z.getvalue())
print('ok')
