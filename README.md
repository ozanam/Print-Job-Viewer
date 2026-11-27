# Print Job Viewer

Open and compare spool files, check what settings are set, make changes to a spool file and reprint.

**Print File Viewer** is a portable, self-contained viewer and inspector for printer job files, written in Rust. It's a single executable for Windows and macOS: no installer, no runtime, no Ghostscript and no DLLs. Copy it anywhere (including a USB stick) and run it.

**Downloads:** see [Releases](https://github.com/ozanam/Print-Job-Viewer/releases): `PrintFileViewer.exe` for Windows 10/11 and `PrintFileViewer-macOS-*.zip` for macOS 11+ (Apple Silicon and Intel).

## What it opens

| Format | Notes |
| --- | --- |
| **PCL 6 / PCL XL** (PCL 6 Enhanced) | Own interpreter: paths, fonts (downloaded TrueType and bitmap), images, raster patterns |
| **PCL 5** (PCL 5e / 5c) | Text with resident and downloaded (bitmap and TrueType) fonts, raster graphics (all common compressions), rectangles and shading, colour palettes, macros and overlays |
| **HP-GL/2** | Embedded in PCL 5, or stand-alone plot files (`.plt`, `.hpgl`) |
| **PostScript** and **EPS** | Own Level 2/3 interpreter with Type 1, Type 3, Type 42 and CID fonts |
| **Windows EMF spool files** | The "NT EMF" spool data type, one page per metafile, with embedded fonts and the job's DEVMODE. Also plain `.emf` files. |
| **XPS / OpenXPS** | Also the spool format of Windows v4 printer drivers. Embedded (obfuscated) fonts, images, paths, transforms and clips. |
| **Windows `.SHD` shadow files** | Read automatically alongside the matching `.SPL` spool file |
| **PJL** headers | Every command, decoded |

## Features

**View**
- Thumbnails (with a colour-page marker), zoom, fit width and fit page
- Click any object on the page to see what it is (text, shape, line or image), its colour, size and position, and the exact command that drew it. You can jump to that command or to its bytes.
- **Find text** (Ctrl+F) with every match highlighted; copy or export the page text. For Windows driver output, downloaded font subsets are matched against installed fonts, so text that has no character codes can still be read.
- Rulers, cursor position in mm, points and printer dots, and a **measure** tool (M; hold Shift for straight lines)
- Turn text, graphics and images on or off separately
- Show the printable area (4.2 mm margin) and the actual ink extent
- Dark mode

**Info**: everything known about the job:
- File, detected format, job structure (language segments with offsets)
- **Windows job record from the `.SHD` file**: owner, computer, document name, printer, driver, port, print processor, data type, submit time, spooler page count. It also decodes the full **driver settings (DEVMODE)**: paper, tray, duplex, colour, copies, collate, resolution, media type and N-up.
- **Settings checks**: flags settings that contradict each other or the content. Examples are stapling or duplex on a one-page job, paper size or copy-count mismatches between PJL, the driver and the page data, mono jobs containing colour, colour jobs that are really black and white, blank pages, content too close to the paper edge, and an `.SHD` file that doesn't match its spool file.
- **Toner coverage**: estimated C/M/Y/K coverage per page, plus colour versus black-and-white page counts
- PJL settings and raw commands; PCL XL session, page, font and image details; PCL 5 job settings, soft fonts and command statistics; PostScript DSC comments, page device requests and fonts; XPS print ticket; EMF fonts and records
- Filter, copy all, export

**Commands**: decoded listing for PCL XL, PCL 5, HP-GL/2, PostScript painting operators and EMF records, with file offsets. Double-click a command to highlight what it drew.

**Hex**: the raw bytes, with the selected command's bytes highlighted, and go-to-offset.

**Compare**: open a second job and compare them:
- *Differences*: an overlay where red is only in A and blue is only in B, with the share of the page that changed, and the list of pages that differ
- *Side by side*
- *Job settings*: every Info value that differs between the two jobs

**Output**
- **Export to PDF** (vector), PNG or text
- **Print** the rendered pages to any installed printer
- **Send the original job unchanged** to an installed printer queue (RAW) or to a network printer's raw port (9100 / JetDirect)
- **Edit the PJL header** (copies, duplex, tray, output bin, mono, hold/PIN, stapling…), then save the edited job or send it straight to a printer. The rest of the job is kept byte-for-byte.

**Tools**
- **Batch report**: scan a folder (optionally with subfolders) and get one row per job: format, pages, colour pages, paper, duplex, copies, user, job name, application, computer, printer, driver, submit time, coverage and checks. Sort it, double-click to open a job, export to **CSV** or **JSON**.
- **Watch spool folder**: lists jobs as they arrive in `C:\Windows\System32\spool\PRINTERS` (or any folder), and can **keep a copy of every job** with its `.SHD` before Windows deletes it. Reading the spool folder needs administrator rights.
- Recent files; the window size and view options are remembered. For fully portable use, create an empty `PrintFileViewer.settings` file next to the program and settings are stored there instead of in the user profile.

## Command line

```text
printfileviewer <file>                                  open the file in the viewer
printfileviewer --info <file>                           print all job information
printfileviewer --text <file>                           print the text on each page
printfileviewer --render <file> <out.png> [page] [dpi]  render a page
printfileviewer --pdf <file> <out.pdf>                  convert to PDF
printfileviewer --batch <folder> <out.csv|out.json> [--recursive]
printfileviewer --send <file> <host[:port]>             send a job unchanged to a network printer
```

These work from cmd and PowerShell on Windows as well as from a terminal on macOS (inside the app bundle: `"Print File Viewer.app/Contents/MacOS/PrintFileViewer" --info job.prn`).

## Building

Requires Rust (stable).

```bash
cargo build --release
```

Cross-compiling the Windows `.exe` from macOS (rustup + mingw-w64 from Homebrew):

```bash
PATH=/opt/homebrew/opt/rustup/bin:$PATH rustup run stable cargo build --release --target x86_64-pc-windows-gnu
```

The result is statically linked and only imports DLLs that ship with Windows 10/11. It embeds the icon and version information.

macOS app bundle (universal, macOS 11+, with file associations):

```bash
./scripts/build-macos.sh
```

The app is ad-hoc signed but not notarized: on another Mac, the first time you open it, right-click the app and choose **Open**.

The GitHub Actions workflow (`.github/workflows/build.yml`) builds Windows and macOS binaries on every push.

Test files can be regenerated with the scripts in `samples/` (`make_pclxl_sample.py`, `make_pcl5_sample.py`, `make_emf_xps_samples.py`).

## Known limitations

- Smooth shadings and XPS gradients are approximated with an average colour. Transparency is approximated by blending with white.
- CFF fonts (PostScript FontType 2 / CIDFontType 0) and PCL Intellifont soft fonts are substituted.
- ROP3 raster operations are approximated: only the common transparent-mask cases are handled.
- EMF+ (GDI+) records are not drawn; the EMF copy that dual-format files include is drawn instead.
- EMF text stored as glyph numbers needs the original font to be installed or embedded in the spool file.
- TIFF and HD Photo images in XPS are not decoded.

## Source layout

| File | Purpose |
| --- | --- |
| `src/main.rs` | startup and command-line commands |
| `src/app.rs` | the user interface |
| `src/loader.rs` | file loading, format detection, analysis |
| `src/pjl.rs` | PJL parsing and job splitting |
| `src/pclxl.rs` | PCL XL interpreter |
| `src/pcl5.rs`, `src/hpgl.rs` | PCL 5 and HP-GL/2 interpreters |
| `src/ps/` | PostScript interpreter |
| `src/emf.rs` | EMF and EMF spool player |
| `src/xps.rs`, `src/zip.rs`, `src/xml.rs` | XPS reader |
| `src/shd.rs` | Windows `.SHD` shadow files and DEVMODE |
| `src/analyze.rs` | coverage, colour detection and settings checks |
| `src/summary.rs` | job summaries and the batch report |
| `src/pdf.rs` | PDF export |
| `src/output.rs` | printing and sending to printers |
| `src/fonts.rs`, `src/imgdec.rs` | fonts, font identification, image decoding |
| `src/render.rs`, `src/doc.rs`, `src/geom.rs` | rasterizer, page model, geometry |
