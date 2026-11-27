//! PCL XL (PCL 6 Enhanced) parser and interpreter.

use crate::doc::*;
use crate::fonts::{self, SfntInfo, SysFont};
use crate::geom::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, PathBuilder, Transform};

#[derive(Clone, Copy, PartialEq, Debug)]
enum DT {
    U8,
    U16,
    U32,
    S16,
    S32,
    R32,
}

impl DT {
    fn from(n: u8) -> DT {
        match n {
            0 => DT::U8,
            1 => DT::U16,
            2 => DT::U32,
            3 => DT::S16,
            4 => DT::S32,
            _ => DT::R32,
        }
    }
    fn size(self) -> usize {
        match self {
            DT::U8 => 1,
            DT::U16 | DT::S16 => 2,
            _ => 4,
        }
    }
}

#[derive(Clone, Debug)]
enum Val {
    Scalar(f64, DT),
    Array(Vec<f64>, DT),
    Xy(f64, f64),
    Box([f64; 4]),
}

impl Val {
    fn num(&self) -> Option<f64> {
        match self {
            Val::Scalar(v, _) => Some(*v),
            Val::Array(a, _) => a.first().copied(),
            Val::Xy(x, _) => Some(*x),
            _ => None,
        }
    }
    fn xy(&self) -> Option<(f64, f64)> {
        match self {
            Val::Xy(x, y) => Some((*x, *y)),
            Val::Scalar(v, _) => Some((*v, *v)),
            Val::Array(a, _) if a.len() >= 2 => Some((a[0], a[1])),
            _ => None,
        }
    }
    fn bx(&self) -> Option<[f64; 4]> {
        match self {
            Val::Box(b) => Some(*b),
            Val::Array(a, _) if a.len() >= 4 => Some([a[0], a[1], a[2], a[3]]),
            _ => None,
        }
    }
    fn arr(&self) -> Option<Vec<f64>> {
        match self {
            Val::Array(a, _) => Some(a.clone()),
            Val::Scalar(v, _) => Some(vec![*v]),
            Val::Xy(x, y) => Some(vec![*x, *y]),
            Val::Box(b) => Some(b.to_vec()),
        }
    }
    fn is_int_dt(&self) -> bool {
        !matches!(self, Val::Scalar(_, DT::R32) | Val::Array(_, DT::R32))
    }
    fn bytes(&self) -> Vec<u8> {
        self.arr().unwrap_or_default().iter().map(|v| *v as u8).collect()
    }
    fn string(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).trim_end_matches(['\0', ' ']).to_string()
    }
    fn fmt(&self) -> String {
        fn n(v: f64) -> String {
            if v.fract() == 0.0 && v.abs() < 1e12 {
                format!("{}", v as i64)
            } else {
                format!("{v:.4}")
            }
        }
        match self {
            Val::Scalar(v, _) => n(*v),
            Val::Xy(x, y) => format!("({}, {})", n(*x), n(*y)),
            Val::Box(b) => format!("[{}, {}, {}, {}]", n(b[0]), n(b[1]), n(b[2]), n(b[3])),
            Val::Array(a, dt) => {
                if *dt == DT::U8 && !a.is_empty() && a.iter().all(|&c| (32.0..127.0).contains(&c)) {
                    format!("\"{}\"", a.iter().map(|&c| c as u8 as char).collect::<String>())
                } else if a.len() > 16 {
                    format!("[{} … ({} items)]", a[..16].iter().map(|v| n(*v)).collect::<Vec<_>>().join(" "), a.len())
                } else {
                    format!("[{}]", a.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" "))
                }
            }
        }
    }
}

// Attribute ids
#[allow(dead_code)]
mod a {
    pub const PALETTE_DEPTH: u16 = 2;
    pub const COLOR_SPACE: u16 = 3;
    pub const NULL_BRUSH: u16 = 4;
    pub const NULL_PEN: u16 = 5;
    pub const PALETTE_DATA: u16 = 6;
    pub const PATTERN_SELECT_ID: u16 = 8;
    pub const GRAY_LEVEL: u16 = 9;
    pub const RGB_COLOR: u16 = 11;
    pub const PATTERN_ORIGIN: u16 = 12;
    pub const NEW_DESTINATION_SIZE: u16 = 13;
    pub const PRIMARY_ARRAY: u16 = 14;
    pub const MEDIA_DESTINATION: u16 = 36;
    pub const MEDIA_SIZE: u16 = 37;
    pub const MEDIA_SOURCE: u16 = 38;
    pub const MEDIA_TYPE: u16 = 39;
    pub const ORIENTATION: u16 = 40;
    pub const PAGE_ANGLE: u16 = 41;
    pub const PAGE_ORIGIN: u16 = 42;
    pub const PAGE_SCALE: u16 = 43;
    pub const ROP3: u16 = 44;
    pub const TX_MODE: u16 = 45;
    pub const CUSTOM_MEDIA_SIZE: u16 = 47;
    pub const CUSTOM_MEDIA_SIZE_UNITS: u16 = 48;
    pub const PAGE_COPIES: u16 = 49;
    pub const SIMPLEX_PAGE_MODE: u16 = 52;
    pub const DUPLEX_PAGE_MODE: u16 = 53;
    pub const DUPLEX_PAGE_SIDE: u16 = 54;
    pub const ARC_DIRECTION: u16 = 65;
    pub const BOUNDING_BOX: u16 = 66;
    pub const DASH_OFFSET: u16 = 67;
    pub const ELLIPSE_DIMENSION: u16 = 68;
    pub const END_POINT: u16 = 69;
    pub const FILL_MODE: u16 = 70;
    pub const LINE_CAP_STYLE: u16 = 71;
    pub const LINE_JOIN_STYLE: u16 = 72;
    pub const MITER_LENGTH: u16 = 73;
    pub const LINE_DASH_STYLE: u16 = 74;
    pub const PEN_WIDTH: u16 = 75;
    pub const POINT: u16 = 76;
    pub const NUMBER_OF_POINTS: u16 = 77;
    pub const SOLID_LINE: u16 = 78;
    pub const START_POINT: u16 = 79;
    pub const POINT_TYPE: u16 = 80;
    pub const CONTROL_POINT1: u16 = 81;
    pub const CONTROL_POINT2: u16 = 82;
    pub const CLIP_REGION: u16 = 83;
    pub const COLOR_DEPTH: u16 = 98;
    pub const BLOCK_HEIGHT: u16 = 99;
    pub const COLOR_MAPPING: u16 = 100;
    pub const COMPRESS_MODE: u16 = 101;
    pub const DESTINATION_BOX: u16 = 102;
    pub const DESTINATION_SIZE: u16 = 103;
    pub const PATTERN_DEFINE_ID: u16 = 105;
    pub const SOURCE_HEIGHT: u16 = 107;
    pub const SOURCE_WIDTH: u16 = 108;
    pub const START_LINE: u16 = 109;
    pub const PAD_BYTES_MULTIPLE: u16 = 110;
    pub const COMMENT_DATA: u16 = 129;
    pub const DATA_ORG: u16 = 130;
    pub const MEASURE: u16 = 134;
    pub const SOURCE_TYPE: u16 = 136;
    pub const UNITS_PER_MEASURE: u16 = 137;
    pub const STREAM_NAME: u16 = 139;
    pub const ERROR_REPORT: u16 = 143;
    pub const CHAR_ANGLE: u16 = 161;
    pub const CHAR_CODE: u16 = 162;
    pub const CHAR_SCALE: u16 = 164;
    pub const CHAR_SHEAR: u16 = 165;
    pub const CHAR_SIZE: u16 = 166;
    pub const FONT_HEADER_LENGTH: u16 = 167;
    pub const FONT_NAME: u16 = 168;
    pub const FONT_FORMAT: u16 = 169;
    pub const SYMBOL_SET: u16 = 170;
    pub const TEXT_DATA: u16 = 171;
    pub const X_SPACING_DATA: u16 = 175;
    pub const Y_SPACING_DATA: u16 = 176;
    pub const CHAR_BOLD_VALUE: u16 = 177;
}

fn attr_name(id: u16) -> &'static str {
    match id {
        2 => "PaletteDepth",
        3 => "ColorSpace",
        4 => "NullBrush",
        5 => "NullPen",
        6 => "PaletteData",
        7 => "PaletteIndex",
        8 => "PatternSelectID",
        9 => "GrayLevel",
        11 => "RGBColor",
        12 => "PatternOrigin",
        13 => "NewDestinationSize",
        14 => "PrimaryArray",
        15 => "PrimaryDepth",
        17 => "ColorimetricColorSpace",
        18 => "XYChromaticities",
        19 => "WhiteReferencePoint",
        20 => "CRGBMinMax",
        21 => "GammaGain",
        29 => "AllObjectTypes",
        30 => "TextObjects",
        31 => "VectorObjects",
        32 => "RasterObjects",
        33 => "DeviceMatrix",
        34 => "DitherMatrixDataType",
        35 => "DitherOrigin",
        36 => "MediaDestination",
        37 => "MediaSize",
        38 => "MediaSource",
        39 => "MediaType",
        40 => "Orientation",
        41 => "PageAngle",
        42 => "PageOrigin",
        43 => "PageScale",
        44 => "ROP3",
        45 => "TxMode",
        47 => "CustomMediaSize",
        48 => "CustomMediaSizeUnits",
        49 => "PageCopies",
        50 => "DitherMatrixSize",
        51 => "DitherMatrixDepth",
        52 => "SimplexPageMode",
        53 => "DuplexPageMode",
        54 => "DuplexPageSide",
        65 => "ArcDirection",
        66 => "BoundingBox",
        67 => "DashOffset",
        68 => "EllipseDimension",
        69 => "EndPoint",
        70 => "FillMode",
        71 => "LineCapStyle",
        72 => "LineJoinStyle",
        73 => "MiterLength",
        74 => "LineDashStyle",
        75 => "PenWidth",
        76 => "Point",
        77 => "NumberOfPoints",
        78 => "SolidLine",
        79 => "StartPoint",
        80 => "PointType",
        81 => "ControlPoint1",
        82 => "ControlPoint2",
        83 => "ClipRegion",
        84 => "ClipMode",
        98 => "ColorDepth",
        99 => "BlockHeight",
        100 => "ColorMapping",
        101 => "CompressMode",
        102 => "DestinationBox",
        103 => "DestinationSize",
        104 => "PatternPersistence",
        105 => "PatternDefineID",
        107 => "SourceHeight",
        108 => "SourceWidth",
        109 => "StartLine",
        110 => "PadBytesMultiple",
        111 => "BlockByteLength",
        115 => "NumberOfScanLines",
        120 => "ColorTreatment",
        129 => "CommentData",
        130 => "DataOrg",
        134 => "Measure",
        136 => "SourceType",
        137 => "UnitsPerMeasure",
        138 => "QueryKey",
        139 => "StreamName",
        140 => "StreamDataLength",
        143 => "ErrorReport",
        145 => "IOReadTimeOut",
        161 => "CharAngle",
        162 => "CharCode",
        163 => "CharDataSize",
        164 => "CharScale",
        165 => "CharShear",
        166 => "CharSize",
        167 => "FontHeaderLength",
        168 => "FontName",
        169 => "FontFormat",
        170 => "SymbolSet",
        171 => "TextData",
        172 => "CharSubModeArray",
        173 => "WritingMode",
        175 => "XSpacingData",
        176 => "YSpacingData",
        177 => "CharBoldValue",
        _ => "",
    }
}

fn op_name(op: u8) -> &'static str {
    match op {
        0x41 => "BeginSession",
        0x42 => "EndSession",
        0x43 => "BeginPage",
        0x44 => "EndPage",
        0x46 => "VendorUnique",
        0x47 => "Comment",
        0x48 => "OpenDataSource",
        0x49 => "CloseDataSource",
        0x4A => "EchoComment",
        0x4B => "Query",
        0x4C => "Diagnostic3",
        0x4F => "BeginFontHeader",
        0x50 => "ReadFontHeader",
        0x51 => "EndFontHeader",
        0x52 => "BeginChar",
        0x53 => "ReadChar",
        0x54 => "EndChar",
        0x55 => "RemoveFont",
        0x56 => "SetCharAttributes",
        0x57 => "SetDefaultGS",
        0x58 => "SetColorTreatment",
        0x5B => "BeginStream",
        0x5C => "ReadStream",
        0x5D => "EndStream",
        0x5E => "ExecStream",
        0x5F => "RemoveStream",
        0x60 => "PopGS",
        0x61 => "PushGS",
        0x62 => "SetClipReplace",
        0x63 => "SetBrushSource",
        0x64 => "SetCharAngle",
        0x65 => "SetCharScale",
        0x66 => "SetCharShear",
        0x67 => "SetClipIntersect",
        0x68 => "SetClipRectangle",
        0x69 => "SetClipToPage",
        0x6A => "SetColorSpace",
        0x6B => "SetCursor",
        0x6C => "SetCursorRel",
        0x6D => "SetHalftoneMethod",
        0x6E => "SetFillMode",
        0x6F => "SetFont",
        0x70 => "SetLineDash",
        0x71 => "SetLineCap",
        0x72 => "SetLineJoin",
        0x73 => "SetMiterLimit",
        0x74 => "SetPageDefaultCTM",
        0x75 => "SetPageOrigin",
        0x76 => "SetPageRotation",
        0x77 => "SetPageScale",
        0x78 => "SetPaintTxMode",
        0x79 => "SetPenSource",
        0x7A => "SetPenWidth",
        0x7B => "SetROP",
        0x7C => "SetSourceTxMode",
        0x7D => "SetCharBoldValue",
        0x7E => "SetNeutralAxis",
        0x7F => "SetClipMode",
        0x80 => "SetPathToClip",
        0x81 => "SetCharSubMode",
        0x82 => "BeginUserDefinedLineCap",
        0x83 => "EndUserDefinedLineCap",
        0x84 => "CloseSubPath",
        0x85 => "NewPath",
        0x86 => "PaintPath",
        0x91 => "ArcPath",
        0x92 => "SetColorTrapping",
        0x93 => "BezierPath",
        0x94 => "SetAdaptiveHalftoning",
        0x95 => "BezierRelPath",
        0x96 => "Chord",
        0x97 => "ChordPath",
        0x98 => "Ellipse",
        0x99 => "EllipsePath",
        0x9B => "LinePath",
        0x9D => "LineRelPath",
        0x9E => "Pie",
        0x9F => "PiePath",
        0xA0 => "Rectangle",
        0xA1 => "RectanglePath",
        0xA2 => "RoundRectangle",
        0xA3 => "RoundRectanglePath",
        0xA8 => "Text",
        0xA9 => "TextPath",
        0xB0 => "BeginImage",
        0xB1 => "ReadImage",
        0xB2 => "EndImage",
        0xB3 => "BeginRastPattern",
        0xB4 => "ReadRastPattern",
        0xB5 => "EndRastPattern",
        0xB6 => "BeginScan",
        0xB8 => "EndScan",
        0xB9 => "ScanLineRel",
        0xBA => "PassThrough",
        _ => "Unknown",
    }
}

fn enum_name(attr: u16, v: f64) -> Option<&'static str> {
    let i = v as i64;
    let t: &[&str] = match attr {
        a::ORIENTATION => &["ePortraitOrientation", "eLandscapeOrientation", "eReversePortrait", "eReverseLandscape", "eDefaultOrientation"],
        a::MEDIA_SIZE => &MEDIA_NAMES,
        a::MEDIA_SOURCE => &["eDefaultSource", "eAutoSelect", "eManualFeed", "eMultiPurposeTray", "eUpperCassette", "eLowerCassette", "eEnvelopeTray", "eThirdCassette"],
        a::MEDIA_DESTINATION => &["eDefaultDestination", "eFaceDownBin", "eFaceUpBin", "eJobOffsetBin"],
        a::DUPLEX_PAGE_MODE => &["eDuplexHorizontalBinding", "eDuplexVerticalBinding"],
        a::DUPLEX_PAGE_SIDE => &["eFrontMediaSide", "eBackMediaSide"],
        a::SIMPLEX_PAGE_MODE => &["eSimplexFrontSide"],
        a::MEASURE => &["eInch", "eMillimeter", "eTenthsOfAMillimeter"],
        a::COLOR_SPACE => &["eBiLevel", "eGray", "eRGB", "eCMY", "eCIELab", "eCRGB", "eSRGB"],
        a::COLOR_MAPPING => &["eDirectPixel", "eIndexedPixel", "eDirectPlane"],
        a::COLOR_DEPTH => &["e1Bit", "e4Bit", "e8Bit"],
        a::COMPRESS_MODE => &["eNoCompression", "eRLECompression", "eJPEGCompression", "eDeltaRowCompression"],
        a::DATA_ORG => &["eBinaryHighByteFirst", "eBinaryLowByteFirst"],
        a::FILL_MODE => &["eNonZeroWinding", "eEvenOdd"],
        a::LINE_CAP_STYLE => &["eButtCap", "eRoundCap", "eSquareCap", "eTriangleCap"],
        a::LINE_JOIN_STYLE => &["eMiterJoin", "eRoundJoin", "eBevelJoin", "eNoJoin"],
        a::ARC_DIRECTION => &["eClockWise", "eCounterClockWise"],
        a::CLIP_REGION => &["eInterior", "eExterior"],
        a::TX_MODE => &["eOpaque", "eTransparent"],
        a::ERROR_REPORT => &["eNoReporting", "eBackChannel", "eErrorPage", "eBackChAndErrPage", "eNWBackChannel", "eNWErrorPage", "eNWBackChAndErrPage"],
        a::POINT_TYPE => &["eUByte", "eSByte", "eUInt16", "eSInt16"],
        a::SOURCE_TYPE => &["eDefaultDataSource"],
        a::NULL_BRUSH | a::NULL_PEN | a::SOLID_LINE => &["0"],
        _ => return None,
    };
    t.get(i as usize).copied().filter(|s| *s != "0")
}

const MEDIA_NAMES: [&str; 22] = [
    "eLetterPaper", "eLegalPaper", "eA4Paper", "eExecPaper", "eLedgerPaper", "eA3Paper", "eCOM10Envelope", "eMonarchEnvelope", "eC5Envelope", "eDLEnvelope", "eJB4Paper",
    "eJB5Paper", "eB5Envelope", "eB5Paper", "eJPostcard", "eJDoublePostcard", "eA5Paper", "eA6Paper", "eJB6Paper", "eJIS8KPaper", "eJIS16KPaper", "eJISExecPaper",
];

/// Media size in points for enum value.
fn media_size_enum(i: i64) -> (f64, f64, &'static str) {
    match i {
        0 => (612.0, 792.0, "Letter"),
        1 => (612.0, 1008.0, "Legal"),
        2 => (595.28, 841.89, "A4"),
        3 => (522.0, 756.0, "Executive"),
        4 => (792.0, 1224.0, "Ledger"),
        5 => (841.89, 1190.55, "A3"),
        6 => (297.0, 684.0, "COM10 Envelope"),
        7 => (279.0, 540.0, "Monarch Envelope"),
        8 => (459.21, 649.13, "C5 Envelope"),
        9 => (311.81, 623.62, "DL Envelope"),
        10 => (728.5, 1031.81, "JIS B4"),
        11 => (515.91, 728.5, "JIS B5"),
        12 => (498.9, 708.66, "B5 Envelope"),
        13 => (498.9, 708.66, "ISO B5"),
        14 => (283.46, 419.53, "Japanese Postcard"),
        15 => (566.93, 419.53, "Japanese Double Postcard"),
        16 => (419.53, 595.28, "A5"),
        17 => (297.64, 419.53, "A6"),
        18 => (362.83, 515.91, "JIS B6"),
        19 => (765.35, 1105.51, "8K"),
        20 => (552.76, 765.35, "16K"),
        21 => (612.0, 936.0, "JIS Executive"),
        _ => (612.0, 792.0, "Letter (unknown)"),
    }
}

fn media_size_name(n: &str) -> Option<(f64, f64)> {
    let u = n.trim().to_ascii_uppercase();
    let i = match u.as_str() {
        "LETTER" => 0,
        "LEGAL" => 1,
        "A4" => 2,
        "EXEC" | "EXECUTIVE" => 3,
        "LEDGER" | "TABLOID" => 4,
        "A3" => 5,
        "COM10" => 6,
        "MONARCH" => 7,
        "C5" => 8,
        "DL" => 9,
        "JB4" | "B4" => 10,
        "JB5" | "B5" => 11,
        "B5ENV" => 12,
        "JPOST" => 14,
        "JDOUBLEPOST" | "JPOSTD" => 15,
        "A5" => 16,
        "A6" => 17,
        "JB6" => 18,
        "8K" => 19,
        "16K" => 20,
        _ => return None,
    };
    let (w, h, _) = media_size_enum(i);
    Some((w, h))
}

#[derive(Clone)]
enum XPaint {
    Null,
    Solid(Rgb),
    Pattern(i64, (f64, f64)),
}

#[derive(Clone)]
struct FontSel {
    name: Vec<u8>,
    size: f64,
    symset: u32,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    brush: XPaint,
    pen: XPaint,
    pen_width: f64,
    cap: LineCap,
    join: LineJoin,
    miter: f64,
    dash: Option<(Vec<f32>, f32)>,
    even_odd: bool,
    clip: Clip,
    color_space: u8,
    palette: Option<Vec<u8>>,
    font: Option<FontSel>,
    char_angle: f64,
    char_scale: (f64, f64),
    char_shear: (f64, f64),
    char_bold: f64,
    src_tx: u8,
    paint_tx: u8,
    rop: u8,
}

impl Gs {
    fn new(ctm: Matrix) -> Gs {
        Gs {
            ctm,
            brush: XPaint::Solid(Rgb::BLACK),
            pen: XPaint::Solid(Rgb::BLACK),
            pen_width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 10.0,
            dash: None,
            even_odd: false,
            clip: None,
            color_space: 1,
            palette: None,
            font: None,
            char_angle: 0.0,
            char_scale: (1.0, 1.0),
            char_shear: (0.0, 0.0),
            char_bold: 0.0,
            src_tx: 0,
            paint_tx: 0,
            rop: 252,
        }
    }
}

enum XGlyphs {
    TrueType { sfnt: SfntInfo, by_code: HashMap<u32, (u16, Option<f64>)>, by_gid: HashMap<u16, Vec<u8>> },
    Bitmap { res: (f64, f64), chars: HashMap<u32, BitmapChar> },
}

struct BitmapChar {
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    bits: Vec<u8>,
}

struct XFont {
    kind: XGlyphs,
    symset: u32,
    header_len: usize,
    /// Installed font this subset was matched to (glyph → character), and the
    /// number of glyphs that were available at the last attempt.
    ident: Option<Arc<fonts::Identified>>,
    ident_tried: usize,
}

struct ImageState {
    mapping: u8,
    depth: u8,
    sw: u32,
    sh: u32,
    dw: f64,
    dh: f64,
    origin: (f64, f64),
    pattern_id: Option<i64>,
    rows: Vec<u8>, // for patterns: accumulated decoded rgba
    seed: Vec<u8>,
}

struct PatternDef {
    image: Arc<RasterImage>,
    dw: f64,
    dh: f64,
}

#[derive(Default)]
struct PageInfo {
    media: String,
    orientation: String,
    source: String,
    mtype: String,
    destination: String,
    duplex: String,
    copies: String,
}

pub struct XlInterp<'d> {
    doc: &'d mut Document,
    le: bool,
    gs: Gs,
    gs_stack: Vec<Gs>,
    path: PathAcc,
    cursor: (f64, f64), // device (page points)
    page: Option<Page>,
    default_ctm: Matrix,
    units: (f64, f64),
    fonts: HashMap<Vec<u8>, XFont>,
    font_build: Option<(Vec<u8>, Vec<u8>, u32)>,
    char_build: Option<(Vec<u8>,)>,
    image: Option<ImageState>,
    patterns: HashMap<i64, PatternDef>,
    streams: HashMap<Vec<u8>, Vec<u8>>,
    stream_build: Option<(Vec<u8>, Vec<u8>)>,
    op_counts: BTreeMap<&'static str, usize>,
    page_infos: Vec<PageInfo>,
    cur_page_info: PageInfo,
    session: InfoSection,
    comments: Vec<String>,
    font_uses: BTreeMap<String, usize>,
    image_stats: BTreeMap<String, usize>,
    glyph_cache: HashMap<(Vec<u8>, u32, u32), Arc<RasterImage>>,
    sys_fonts: HashMap<Vec<u8>, Arc<SysFont>>,
    depth: u32,
    header: String,
}

fn rd_u16(d: &[u8], p: usize, le: bool) -> Option<u16> {
    let b = [*d.get(p)?, *d.get(p + 1)?];
    Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
}
fn rd_u32(d: &[u8], p: usize, le: bool) -> Option<u32> {
    let b = [*d.get(p)?, *d.get(p + 1)?, *d.get(p + 2)?, *d.get(p + 3)?];
    Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
}
fn rd_num(d: &[u8], p: usize, dt: DT, le: bool) -> Option<f64> {
    Some(match dt {
        DT::U8 => *d.get(p)? as f64,
        DT::U16 => rd_u16(d, p, le)? as f64,
        DT::S16 => rd_u16(d, p, le)? as i16 as f64,
        DT::U32 => rd_u32(d, p, le)? as f64,
        DT::S32 => rd_u32(d, p, le)? as i32 as f64,
        DT::R32 => f32::from_bits(rd_u32(d, p, le)?) as f64,
    })
}

type Attrs = Vec<(u16, Val)>;

fn get(at: &Attrs, id: u16) -> Option<&Val> {
    at.iter().rev().find(|(i, _)| *i == id).map(|(_, v)| v)
}

impl<'d> XlInterp<'d> {
    pub fn new(doc: &'d mut Document) -> Self {
        XlInterp {
            doc,
            le: true,
            gs: Gs::new(Matrix::scale(72.0 / 600.0, 72.0 / 600.0)),
            gs_stack: Vec::new(),
            path: PathAcc::new(),
            cursor: (0.0, 0.0),
            page: None,
            default_ctm: Matrix::scale(72.0 / 600.0, 72.0 / 600.0),
            units: (600.0, 600.0),
            fonts: HashMap::new(),
            font_build: None,
            char_build: None,
            image: None,
            patterns: HashMap::new(),
            streams: HashMap::new(),
            stream_build: None,
            op_counts: BTreeMap::new(),
            page_infos: Vec::new(),
            cur_page_info: PageInfo::default(),
            session: InfoSection::new("PCL XL session"),
            comments: Vec::new(),
            font_uses: BTreeMap::new(),
            image_stats: BTreeMap::new(),
            glyph_cache: HashMap::new(),
            sys_fonts: HashMap::new(),
            depth: 0,
            header: String::new(),
        }
    }

    /// Interpret one PCL XL stream (starting at the binding byte).
    pub fn run(&mut self, data: &[u8], base_offset: usize) {
        let mut p = 0;
        while p < data.len() && (data[p].is_ascii_whitespace() || data[p] == 0) {
            p += 1;
        }
        if p >= data.len() {
            return;
        }
        let binding = data[p];
        match binding {
            b')' => self.le = true,
            b'(' => self.le = false,
            b'@' => {
                self.doc.warn("ASCII-binding PCL XL streams are not supported");
                return;
            }
            _ => {
                if self.depth == 0 {
                    self.doc.warn("PCL XL stream header not found");
                }
            }
        }
        if matches!(binding, b')' | b'(') {
            let eol = data[p..].iter().position(|&c| c == b'\n').map(|x| p + x + 1).unwrap_or(data.len());
            let hdr = String::from_utf8_lossy(&data[p..eol]).trim().to_string();
            if self.depth == 0 {
                self.header = hdr.clone();
                self.doc.cmd(base_offset + p, format!("{:08X}  Stream header: {hdr}", base_offset + p));
            }
            p = eol;
        }
        self.tokens(data, p, base_offset);
    }

    fn tokens(&mut self, d: &[u8], mut p: usize, base: usize) {
        let le = self.le;
        let mut vals: Vec<Val> = Vec::new();
        let mut attrs: Attrs = Vec::new();
        let n = d.len();
        let mut stmt_start = p;
        while p < n {
            let t = d[p];
            if vals.is_empty() && attrs.is_empty() && !matches!(t, 0x00 | 0x09..=0x0D | 0x20) {
                stmt_start = p;
            }
            match t {
                0x00 | 0x09..=0x0D | 0x20 => p += 1,
                0x1B => break,
                0xC0..=0xC5 => {
                    let dt = DT::from(t - 0xC0);
                    let Some(v) = rd_num(d, p + 1, dt, le) else { break };
                    vals.push(Val::Scalar(v, dt));
                    p += 1 + dt.size();
                }
                0xC8..=0xCD => {
                    let dt = DT::from(t - 0xC8);
                    let lt = d.get(p + 1).copied().unwrap_or(0);
                    let (len, hl) = match lt {
                        0xC0 => (d.get(p + 2).copied().unwrap_or(0) as usize, 2),
                        0xC1 => (rd_u16(d, p + 2, le).unwrap_or(0) as usize, 3),
                        0xC2 => (rd_u32(d, p + 2, le).unwrap_or(0) as usize, 5),
                        _ => {
                            self.doc.warn(format!("Bad array length tag at offset {:#X}", base + p));
                            break;
                        }
                    };
                    let mut q = p + 1 + hl;
                    if q + len * dt.size() > n {
                        self.doc.warn("Truncated PCL XL array");
                        break;
                    }
                    let mut arr = Vec::with_capacity(len);
                    for _ in 0..len {
                        arr.push(rd_num(d, q, dt, le).unwrap_or(0.0));
                        q += dt.size();
                    }
                    vals.push(Val::Array(arr, dt));
                    p = q;
                }
                0xD0..=0xD5 => {
                    let dt = DT::from(t - 0xD0);
                    let (Some(x), Some(y)) = (rd_num(d, p + 1, dt, le), rd_num(d, p + 1 + dt.size(), dt, le)) else { break };
                    vals.push(Val::Xy(x, y));
                    p += 1 + 2 * dt.size();
                }
                0xE0..=0xE5 => {
                    let dt = DT::from(t - 0xE0);
                    let mut b = [0.0; 4];
                    for (i, v) in b.iter_mut().enumerate() {
                        *v = rd_num(d, p + 1 + i * dt.size(), dt, le).unwrap_or(0.0);
                    }
                    vals.push(Val::Box(b));
                    p += 1 + 4 * dt.size();
                }
                0xF8 | 0xF9 => {
                    let (id, l) = if t == 0xF8 { (d.get(p + 1).copied().unwrap_or(0) as u16, 2) } else { (rd_u16(d, p + 1, le).unwrap_or(0), 3) };
                    let v = vals.pop().unwrap_or(Val::Scalar(0.0, DT::U8));
                    attrs.push((id, v));
                    p += l;
                }
                0xFA | 0xFB => {
                    // stray embedded data (should follow an operator)
                    let (len, hl) = if t == 0xFA { (rd_u32(d, p + 1, le).unwrap_or(0) as usize, 5) } else { (d.get(p + 1).copied().unwrap_or(0) as usize, 2) };
                    p += hl + len;
                }
                0x41..=0xBF => {
                    let off = p;
                    p += 1;
                    let mut data: Option<(usize, usize)> = None;
                    if p < n && (d[p] == 0xFA || d[p] == 0xFB) {
                        let (len, hl) = if d[p] == 0xFA { (rd_u32(d, p + 1, le).unwrap_or(0) as usize, 5) } else { (d.get(p + 1).copied().unwrap_or(0) as usize, 2) };
                        let s = p + hl;
                        let e = (s + len).min(n);
                        data = Some((s, e));
                        p = e;
                    }
                    let emb = data.map(|(s, e)| &d[s..e]);
                    if self.depth == 0 {
                        self.log_op(base + off, base + stmt_start, t, &attrs, emb.map(|e| e.len()));
                        if let Some(pg) = self.page.as_mut() {
                            pg.cur_src = (base + stmt_start) as u32;
                        }
                    }
                    *self.op_counts.entry(op_name(t)).or_insert(0) += 1;
                    self.exec(t, &attrs, emb);
                    attrs.clear();
                    vals.clear();
                }
                _ => {
                    self.doc.warn(format!("Unknown PCL XL tag {t:#04X} at offset {:#X}", base + p));
                    p += 1;
                }
            }
        }
        self.finish_page_if_open();
    }

    fn log_op(&mut self, off: usize, stmt: usize, op: u8, attrs: &Attrs, data: Option<usize>) {
        let mut s = format!("{off:08X}  {}", op_name(op));
        if op_name(op) == "Unknown" {
            s.push_str(&format!("({op:#04X})"));
        }
        for (id, v) in attrs {
            let name = attr_name(*id);
            let val = match v {
                Val::Scalar(x, _) => enum_name(*id, *x).map(|s| s.to_string()).unwrap_or_else(|| v.fmt()),
                _ => v.fmt(),
            };
            if name.is_empty() {
                s.push_str(&format!("  attr{id}={val}"));
            } else {
                s.push_str(&format!("  {name}={val}"));
            }
        }
        if let Some(n) = data {
            s.push_str(&format!("  [embedded data: {n} bytes]"));
        }
        self.doc.cmd(stmt, s);
    }

    fn finish_page_if_open(&mut self) {
        if self.depth == 0 {
            if let Some(p) = self.page.take() {
                self.doc.warn("Stream ended without EndPage; page flushed");
                self.doc.pages.push(p);
                self.page_infos.push(std::mem::take(&mut self.cur_page_info));
            }
        }
    }

    // ---- painting helpers ----

    fn push_item(&mut self, item: Item) {
        if let Some(p) = self.page.as_mut() {
            p.push(item);
        }
    }

    fn brush_of(&self, paint: &XPaint) -> Option<Brush> {
        match paint {
            XPaint::Null => None,
            XPaint::Solid(c) => Some(Brush::Solid(*c)),
            XPaint::Pattern(id, origin) => {
                let pd = self.patterns.get(id)?;
                let m = Matrix::scale(pd.dw / pd.image.width as f64, pd.dh / pd.image.height as f64)
                    .then(&Matrix::translate(origin.0, origin.1))
                    .then(&self.gs.ctm);
                Some(Brush::Pattern { image: pd.image.clone(), transform: m.to_ts() })
            }
        }
    }

    fn fill_path(&mut self, acc: &PathAcc) {
        let Some(brush) = self.brush_of(&self.gs.brush.clone()) else { return };
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            let it = Item::Fill { path, transform: Transform::identity(), brush, even_odd: self.gs.even_odd, clip: self.gs.clip.clone() };
            self.push_item(it);
        }
    }

    fn stroke_path(&mut self, acc: &PathAcc) {
        let Some(brush) = self.brush_of(&self.gs.pen.clone()) else { return };
        let Some(inv) = self.gs.ctm.invert() else { return };
        if let Some(path) = acc.to_path(&inv) {
            let style = StrokeStyle { width: self.gs.pen_width as f32, cap: self.gs.cap, join: self.gs.join, miter: self.gs.miter as f32, dash: self.gs.dash.clone() };
            let it = Item::Stroke { path, transform: self.gs.ctm.to_ts(), brush, style, clip: self.gs.clip.clone() };
            self.push_item(it);
        }
    }

    fn paint_current(&mut self) {
        let acc = self.path.clone();
        if acc.is_empty() {
            return;
        }
        self.fill_path(&acc);
        self.stroke_path(&acc);
    }

    fn dev(&self, x: f64, y: f64) -> (f64, f64) {
        self.gs.ctm.apply(x, y)
    }

    fn user_cursor(&self) -> (f64, f64) {
        self.gs.ctm.invert().map(|m| m.apply(self.cursor.0, self.cursor.1)).unwrap_or((0.0, 0.0))
    }

    fn move_to(&mut self, x: f64, y: f64) {
        let (dx, dy) = self.dev(x, y);
        self.path.move_to(dx, dy);
        self.cursor = (dx, dy);
    }

    fn line_to(&mut self, x: f64, y: f64) {
        let (dx, dy) = self.dev(x, y);
        if self.path.cur.is_none() {
            let c = self.cursor;
            self.path.move_to(c.0, c.1);
        }
        self.path.line_to(dx, dy);
        self.cursor = (dx, dy);
    }

    fn curve_to(&mut self, p: [f64; 6]) {
        let (a, b) = self.dev(p[0], p[1]);
        let (c, d) = self.dev(p[2], p[3]);
        let (e, f) = self.dev(p[4], p[5]);
        if self.path.cur.is_none() {
            let cc = self.cursor;
            self.path.move_to(cc.0, cc.1);
        }
        self.path.curve_to(a, b, c, d, e, f);
        self.cursor = (e, f);
    }

    fn rect_path(&mut self, b: [f64; 4]) {
        let (x0, y0, x1, y1) = (b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3]));
        self.move_to(x0, y0);
        self.line_to(x1, y0);
        self.line_to(x1, y1);
        self.line_to(x0, y1);
        self.path.close();
    }

    fn round_rect_path(&mut self, b: [f64; 4], ew: f64, eh: f64) {
        let (x0, y0, x1, y1) = (b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3]));
        let rx = (ew / 2.0).min((x1 - x0) / 2.0);
        let ry = (eh / 2.0).min((y1 - y0) / 2.0);
        let ctm = self.gs.ctm;
        let corner = |acc: &mut PathAcc, cx: f64, cy: f64, a1: f64, connect: bool| {
            let m = Matrix::scale(rx, ry).then(&Matrix::translate(cx, cy)).then(&ctm);
            acc.arc(&m, 0.0, 0.0, 1.0, a1, a1 + 90.0, true, connect);
        };
        corner(&mut self.path, x1 - rx, y0 + ry, 270.0, false);
        corner(&mut self.path, x1 - rx, y1 - ry, 0.0, true);
        corner(&mut self.path, x0 + rx, y1 - ry, 90.0, true);
        corner(&mut self.path, x0 + rx, y0 + ry, 180.0, true);
        self.path.close();
        self.cursor = self.path.cur.unwrap_or(self.cursor);
    }

    /// Arc / chord / pie on the ellipse inscribed in bbox.
    fn arc_path(&mut self, at: &Attrs, kind: u8) {
        let Some(b) = get(at, a::BOUNDING_BOX).and_then(|v| v.bx()) else { return };
        let sp = get(at, a::START_POINT).and_then(|v| v.xy()).unwrap_or((b[2], (b[1] + b[3]) / 2.0));
        let ep = get(at, a::END_POINT).and_then(|v| v.xy()).unwrap_or(sp);
        let ccw = get(at, a::ARC_DIRECTION).and_then(|v| v.num()).unwrap_or(0.0) as i64 == 1;
        let cx = (b[0] + b[2]) / 2.0;
        let cy = (b[1] + b[3]) / 2.0;
        let rx = ((b[2] - b[0]) / 2.0).abs().max(1e-9);
        let ry = ((b[3] - b[1]) / 2.0).abs().max(1e-9);
        let ang = |p: (f64, f64)| ((p.1 - cy) / ry).atan2((p.0 - cx) / rx).to_degrees();
        let a1 = ang(sp);
        let mut a2 = ang(ep);
        if (a2 - a1).abs() < 1e-9 {
            a2 = a1 + if ccw { -360.0 } else { 360.0 };
        }
        // In y-down space, increasing angle is clockwise.
        let m = Matrix::scale(rx, ry).then(&Matrix::translate(cx, cy)).then(&self.gs.ctm);
        let mut tmp = PathAcc::new();
        tmp.arc(&m, 0.0, 0.0, 1.0, a1, a2, !ccw, false);
        match kind {
            0 => {
                // arc: connect to current point
                if self.path.cur.is_some() {
                    if let Some(Seg::Move(x, y)) = tmp.segs.first().copied() {
                        self.path.line_to(x, y);
                        tmp.segs.remove(0);
                    }
                }
                self.path.append(&tmp);
            }
            1 => {
                tmp.close();
                self.path.append(&tmp);
            }
            _ => {
                let (cdx, cdy) = self.dev(cx, cy);
                self.path.move_to(cdx, cdy);
                if let Some(Seg::Move(x, y)) = tmp.segs.first().copied() {
                    self.path.line_to(x, y);
                    tmp.segs.remove(0);
                }
                for s in tmp.segs {
                    self.path.segs.push(s);
                }
                self.path.close();
            }
        }
        if let Some(c) = self.path.cur {
            self.cursor = c;
        }
    }

    fn points_from_data(&self, at: &Attrs, data: Option<&[u8]>) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let Some(d) = data else { return out };
        let n = get(at, a::NUMBER_OF_POINTS).and_then(|v| v.num()).unwrap_or(0.0) as usize;
        let pt = get(at, a::POINT_TYPE).and_then(|v| v.num()).unwrap_or(0.0) as u8;
        let (dt, sz) = match pt {
            0 => (DT::U8, 1),
            1 => (DT::U8, 1),
            2 => (DT::U16, 2),
            _ => (DT::S16, 2),
        };
        let mut p = 0;
        for _ in 0..n {
            let (Some(mut x), Some(mut y)) = (rd_num(d, p, dt, self.le), rd_num(d, p + sz, dt, self.le)) else { break };
            if pt == 1 {
                x = x as u8 as i8 as f64;
                y = y as u8 as i8 as f64;
            }
            out.push((x, y));
            p += 2 * sz;
        }
        out
    }

    fn parse_color(&self, at: &Attrs) -> Option<XPaint> {
        if get(at, a::NULL_BRUSH).is_some() || get(at, a::NULL_PEN).is_some() {
            return Some(XPaint::Null);
        }
        if let Some(id) = get(at, a::PATTERN_SELECT_ID).and_then(|v| v.num()) {
            let origin = get(at, a::PATTERN_ORIGIN).and_then(|v| v.xy()).unwrap_or((0.0, 0.0));
            return Some(XPaint::Pattern(id as i64, origin));
        }
        if let Some(v) = get(at, a::RGB_COLOR).or_else(|| get(at, a::PRIMARY_ARRAY)) {
            let arr = v.arr().unwrap_or_default();
            let scale = if v.is_int_dt() { 255.0 } else { 1.0 };
            if arr.len() >= 3 {
                return Some(XPaint::Solid(Rgb((arr[0] / scale) as f32, (arr[1] / scale) as f32, (arr[2] / scale) as f32).clamp()));
            } else if arr.len() == 1 {
                if let Some(c) = self.palette_color(arr[0] as usize) {
                    return Some(XPaint::Solid(c));
                }
                let g = (arr[0] / scale) as f32;
                return Some(XPaint::Solid(Rgb::gray(g).clamp()));
            }
        }
        if let Some(v) = get(at, a::GRAY_LEVEL) {
            let g = v.num().unwrap_or(0.0);
            if v.is_int_dt() {
                if let Some(c) = self.palette_color(g as usize) {
                    return Some(XPaint::Solid(c));
                }
                return Some(XPaint::Solid(Rgb::gray((g / 255.0) as f32).clamp()));
            }
            return Some(XPaint::Solid(Rgb::gray(g as f32).clamp()));
        }
        None
    }

    fn palette_color(&self, idx: usize) -> Option<Rgb> {
        let pal = self.gs.palette.as_ref()?;
        if self.gs.color_space == 2 || self.gs.color_space == 6 {
            let o = idx * 3;
            if o + 2 < pal.len() {
                return Some(Rgb(pal[o] as f32 / 255.0, pal[o + 1] as f32 / 255.0, pal[o + 2] as f32 / 255.0));
            }
        } else if idx < pal.len() {
            return Some(Rgb::gray(pal[idx] as f32 / 255.0));
        }
        None
    }

    fn page_rect_path(&self) -> PathAcc {
        let mut acc = PathAcc::new();
        let (w, h) = self.page.as_ref().map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
        acc.move_to(-10.0, -10.0);
        acc.line_to(w + 10.0, -10.0);
        acc.line_to(w + 10.0, h + 10.0);
        acc.line_to(-10.0, h + 10.0);
        acc.close();
        acc
    }

    fn set_clip(&mut self, acc: &PathAcc, exterior: bool, intersect: bool) {
        let mut a = acc.clone();
        let mut eo = self.gs.even_odd;
        if exterior {
            let mut r = self.page_rect_path();
            r.append(&a);
            a = r;
            eo = true;
        }
        let Some(path) = a.to_path(&Matrix::identity()) else {
            // empty clip => nothing visible
            let empty = PathBuilder::from_rect(tiny_skia::Rect::from_xywh(-100.0, -100.0, 0.5, 0.5).unwrap());
            self.gs.clip = Some(Arc::new(vec![ClipPath { path: empty, even_odd: false }]));
            return;
        };
        let cp = ClipPath { path, even_odd: eo };
        let mut v: Vec<ClipPath> = if intersect { self.gs.clip.as_ref().map(|c| (**c).clone()).unwrap_or_default() } else { Vec::new() };
        v.push(cp);
        self.gs.clip = Some(Arc::new(v));
    }

    // ---- operator execution ----

    fn exec(&mut self, op: u8, at: &Attrs, data: Option<&[u8]>) {
        match op {
            0x41 => {
                // BeginSession
                let units = get(at, a::UNITS_PER_MEASURE).and_then(|v| v.xy()).unwrap_or((600.0, 600.0));
                let measure = get(at, a::MEASURE).and_then(|v| v.num()).unwrap_or(0.0) as i64;
                let per_inch = match measure {
                    1 => 25.4,
                    2 => 254.0,
                    _ => 1.0,
                };
                self.units = (units.0 * per_inch, units.1 * per_inch);
                if self.depth == 0 {
                    self.session.add("Measure", enum_name(a::MEASURE, measure as f64).unwrap_or("?"));
                    self.session.add("Units per measure", format!("{} × {}", units.0, units.1));
                    self.session.add("Effective resolution", format!("{:.0} × {:.0} units/inch", self.units.0, self.units.1));
                    if let Some(e) = get(at, a::ERROR_REPORT).and_then(|v| v.num()) {
                        self.session.add("Error report", enum_name(a::ERROR_REPORT, e).unwrap_or("?"));
                    }
                }
            }
            0x42 => {}
            0x43 => self.begin_page(at),
            0x44 => {
                if let Some(c) = get(at, a::PAGE_COPIES).and_then(|v| v.num()) {
                    self.cur_page_info.copies = format!("{c}");
                }
                if let Some(mut p) = self.page.take() {
                    let info = std::mem::take(&mut self.cur_page_info);
                    p.label = format!("{} {}", info.media, info.orientation.trim_start_matches('e').trim_end_matches("Orientation"));
                    for (k, v) in [("Media", &info.media), ("Orientation", &info.orientation), ("Source", &info.source), ("Media type", &info.mtype), ("Output", &info.destination), ("Duplex", &info.duplex), ("Copies", &info.copies)] {
                        if !v.is_empty() {
                            p.settings.push((k.to_string(), v.clone()));
                        }
                    }
                    self.doc.pages.push(p);
                    self.page_infos.push(info);
                }
            }
            0x46 => {
                self.doc.warn("VendorUnique operator(s) present (ignored)");
            }
            0x47 => {
                if let Some(v) = get(at, a::COMMENT_DATA) {
                    self.comments.push(v.string());
                }
            }
            0x48 => {
                if let Some(d) = get(at, a::DATA_ORG).and_then(|v| v.num()) {
                    // DataOrg applies to embedded data byte order
                    if self.depth == 0 {
                        self.session.add("Data organisation", enum_name(a::DATA_ORG, d).unwrap_or("?"));
                    }
                }
            }
            0x49 => {}
            0x4F => {
                let name = get(at, a::FONT_NAME).map(|v| v.bytes()).unwrap_or_default();
                let len = get(at, a::FONT_HEADER_LENGTH).and_then(|v| v.num()).unwrap_or(0.0) as u32;
                self.font_build = Some((name, Vec::new(), len));
            }
            0x50 => {
                if let (Some(fb), Some(d)) = (self.font_build.as_mut(), data) {
                    fb.1.extend_from_slice(d);
                }
            }
            0x51 => {
                if let Some((name, hdr, _)) = self.font_build.take() {
                    self.define_font(name, hdr);
                }
            }
            0x52 => {
                let name = get(at, a::FONT_NAME).map(|v| v.bytes()).unwrap_or_default();
                self.char_build = Some((name,));
            }
            0x53 => {
                let code = get(at, a::CHAR_CODE).and_then(|v| v.num()).unwrap_or(0.0) as u32;
                if let (Some((name,)), Some(d)) = (self.char_build.clone(), data) {
                    self.define_char(&name, code, d);
                }
            }
            0x54 => self.char_build = None,
            0x55 => {
                if let Some(n) = get(at, a::FONT_NAME) {
                    self.fonts.remove(&n.bytes());
                }
            }
            0x57 => {
                self.gs = Gs::new(self.default_ctm);
            }
            0x5B => {
                let name = get(at, a::STREAM_NAME).map(|v| v.bytes()).unwrap_or_default();
                self.stream_build = Some((name, Vec::new()));
            }
            0x5C => {
                if let (Some(sb), Some(d)) = (self.stream_build.as_mut(), data) {
                    sb.1.extend_from_slice(d);
                }
            }
            0x5D => {
                if let Some((n, d)) = self.stream_build.take() {
                    self.streams.insert(n, d);
                }
            }
            0x5E => {
                let name = get(at, a::STREAM_NAME).map(|v| v.bytes()).unwrap_or_default();
                if let Some(s) = self.streams.get(&name).cloned() {
                    if self.depth < 8 {
                        self.depth += 1;
                        let le = self.le;
                        self.run(&s, 0);
                        self.le = le;
                        self.depth -= 1;
                    }
                } else {
                    self.doc.warn(format!("ExecStream: undefined stream {}", String::from_utf8_lossy(&name)));
                }
            }
            0x5F => {
                if let Some(n) = get(at, a::STREAM_NAME) {
                    self.streams.remove(&n.bytes());
                }
            }
            0x60 => {
                if let Some(g) = self.gs_stack.pop() {
                    self.gs = g;
                }
            }
            0x61 => {
                if self.gs_stack.len() < 1000 {
                    self.gs_stack.push(self.gs.clone());
                }
            }
            0x62 | 0x67 => {
                let ext = get(at, a::CLIP_REGION).and_then(|v| v.num()).unwrap_or(0.0) as i64 == 1;
                let acc = self.path.clone();
                self.set_clip(&acc, ext, op == 0x67);
            }
            0x63 => {
                if let Some(p) = self.parse_color(at) {
                    self.gs.brush = p;
                }
            }
            0x64 => self.gs.char_angle = get(at, a::CHAR_ANGLE).and_then(|v| v.num()).unwrap_or(0.0),
            0x65 => self.gs.char_scale = get(at, a::CHAR_SCALE).and_then(|v| v.xy()).unwrap_or((1.0, 1.0)),
            0x66 => self.gs.char_shear = get(at, a::CHAR_SHEAR).and_then(|v| v.xy()).unwrap_or((0.0, 0.0)),
            0x68 => {
                let ext = get(at, a::CLIP_REGION).and_then(|v| v.num()).unwrap_or(0.0) as i64 == 1;
                if let Some(b) = get(at, a::BOUNDING_BOX).and_then(|v| v.bx()) {
                    let saved = std::mem::take(&mut self.path);
                    let cur = self.cursor;
                    self.rect_path(b);
                    let acc = std::mem::replace(&mut self.path, saved);
                    self.cursor = cur;
                    self.set_clip(&acc, ext, true);
                }
            }
            0x69 => self.gs.clip = None,
            0x6A => {
                if let Some(cs) = get(at, a::COLOR_SPACE).and_then(|v| v.num()) {
                    self.gs.color_space = cs as u8;
                }
                self.gs.palette = get(at, a::PALETTE_DATA).map(|v| v.bytes());
            }
            0x6B => {
                if let Some((x, y)) = get(at, a::POINT).and_then(|v| v.xy()) {
                    self.move_to(x, y);
                }
            }
            0x6C => {
                if let Some((x, y)) = get(at, a::POINT).and_then(|v| v.xy()) {
                    let (ux, uy) = self.user_cursor();
                    self.move_to(ux + x, uy + y);
                }
            }
            0x6E => self.gs.even_odd = get(at, a::FILL_MODE).and_then(|v| v.num()).unwrap_or(0.0) as i64 == 1,
            0x6F => {
                let name = get(at, a::FONT_NAME).map(|v| v.bytes()).unwrap_or_default();
                let size = get(at, a::CHAR_SIZE).and_then(|v| v.num()).unwrap_or(12.0);
                let symset = get(at, a::SYMBOL_SET).and_then(|v| v.num()).unwrap_or(629.0) as u32;
                let display = String::from_utf8_lossy(&name).trim().to_string();
                let kind = if self.fonts.contains_key(&name) { "downloaded" } else { "resident" };
                *self.font_uses.entry(format!("{display}  ({kind}, symbol set {})", symset_name(symset))).or_insert(0) += 1;
                self.gs.font = Some(FontSel { name, size, symset });
            }
            0x70 => {
                if get(at, a::SOLID_LINE).is_some() {
                    self.gs.dash = None;
                } else if let Some(arr) = get(at, a::LINE_DASH_STYLE).and_then(|v| v.arr()) {
                    let off = get(at, a::DASH_OFFSET).and_then(|v| v.num()).unwrap_or(0.0);
                    let mut d: Vec<f32> = arr.iter().map(|v| *v as f32).collect();
                    if d.len() % 2 == 1 {
                        let c = d.clone();
                        d.extend(c);
                    }
                    self.gs.dash = if d.iter().all(|v| *v <= 0.0) { None } else { Some((d, off as f32)) };
                }
            }
            0x71 => {
                self.gs.cap = match get(at, a::LINE_CAP_STYLE).and_then(|v| v.num()).unwrap_or(0.0) as i64 {
                    1 => LineCap::Round,
                    2 | 3 => LineCap::Square,
                    _ => LineCap::Butt,
                }
            }
            0x72 => {
                self.gs.join = match get(at, a::LINE_JOIN_STYLE).and_then(|v| v.num()).unwrap_or(0.0) as i64 {
                    1 => LineJoin::Round,
                    2 | 3 => LineJoin::Bevel,
                    _ => LineJoin::Miter,
                }
            }
            0x73 => self.gs.miter = get(at, a::MITER_LENGTH).and_then(|v| v.num()).unwrap_or(10.0).max(1.0),
            0x74 => self.gs.ctm = self.default_ctm,
            0x75 => {
                if let Some((x, y)) = get(at, a::PAGE_ORIGIN).and_then(|v| v.xy()) {
                    self.gs.ctm = Matrix::translate(x, y).then(&self.gs.ctm);
                }
            }
            0x76 => {
                if let Some(ang) = get(at, a::PAGE_ANGLE).and_then(|v| v.num()) {
                    self.gs.ctm = Matrix::rotate_deg(ang).then(&self.gs.ctm);
                }
            }
            0x77 => {
                if let Some((x, y)) = get(at, a::PAGE_SCALE).and_then(|v| v.xy()) {
                    self.gs.ctm = Matrix::scale(x, y).then(&self.gs.ctm);
                } else if let Some((ux, uy)) = get(at, a::UNITS_PER_MEASURE).and_then(|v| v.xy()) {
                    let measure = get(at, a::MEASURE).and_then(|v| v.num()).unwrap_or(0.0) as i64;
                    let per_inch = match measure {
                        1 => 25.4,
                        2 => 254.0,
                        _ => 1.0,
                    };
                    let (nx, ny) = (ux * per_inch, uy * per_inch);
                    if nx > 0.0 && ny > 0.0 {
                        self.gs.ctm = Matrix::scale(self.units.0 / nx, self.units.1 / ny).then(&self.gs.ctm);
                    }
                }
            }
            0x78 => self.gs.paint_tx = get(at, a::TX_MODE).and_then(|v| v.num()).unwrap_or(0.0) as u8,
            0x79 => {
                if let Some(p) = self.parse_color(at) {
                    self.gs.pen = p;
                }
            }
            0x7A => self.gs.pen_width = get(at, a::PEN_WIDTH).and_then(|v| v.num()).unwrap_or(1.0),
            0x7B => self.gs.rop = get(at, a::ROP3).and_then(|v| v.num()).unwrap_or(252.0) as u8,
            0x7C => self.gs.src_tx = get(at, a::TX_MODE).and_then(|v| v.num()).unwrap_or(0.0) as u8,
            0x7D => self.gs.char_bold = get(at, a::CHAR_BOLD_VALUE).and_then(|v| v.num()).unwrap_or(0.0),
            0x80 => {
                // SetPathToClip: approximate with page rect
                self.path = self.page_rect_path();
            }
            0x84 => {
                self.path.close();
                if let Some(c) = self.path.cur {
                    self.cursor = c;
                }
            }
            0x85 => self.path.clear(),
            0x86 => self.paint_current(),
            0x91 => self.arc_path(at, 0),
            0x93 | 0x95 => {
                let rel = op == 0x95;
                let pts: Vec<(f64, f64)> = if let (Some(c1), Some(c2), Some(e)) = (
                    get(at, a::CONTROL_POINT1).and_then(|v| v.xy()),
                    get(at, a::CONTROL_POINT2).and_then(|v| v.xy()),
                    get(at, a::END_POINT).and_then(|v| v.xy()),
                ) {
                    vec![c1, c2, e]
                } else {
                    self.points_from_data(at, data)
                };
                for c in pts.chunks(3) {
                    if c.len() < 3 {
                        break;
                    }
                    let (ox, oy) = if rel { self.user_cursor() } else { (0.0, 0.0) };
                    self.curve_to([c[0].0 + ox, c[0].1 + oy, c[1].0 + ox, c[1].1 + oy, c[2].0 + ox, c[2].1 + oy]);
                }
            }
            0x96 | 0x97 => {
                if op == 0x96 {
                    self.path.clear();
                }
                self.arc_path(at, 1);
                if op == 0x96 {
                    self.paint_current();
                }
            }
            0x9E | 0x9F => {
                if op == 0x9E {
                    self.path.clear();
                }
                self.arc_path(at, 2);
                if op == 0x9E {
                    self.paint_current();
                }
            }
            0x98 | 0x99 => {
                if op == 0x98 {
                    self.path.clear();
                }
                if let Some(b) = get(at, a::BOUNDING_BOX).and_then(|v| v.bx()) {
                    let ctm = self.gs.ctm;
                    ellipse_segs(&mut self.path, &ctm, b[0], b[1], b[2], b[3]);
                    let (cx, cy) = (b[0].min(b[2]), b[1].min(b[3]));
                    self.cursor = self.dev(cx, cy);
                }
                if op == 0x98 {
                    self.paint_current();
                }
            }
            0x9B | 0x9D => {
                let rel = op == 0x9D;
                let pts: Vec<(f64, f64)> = if let Some(e) = get(at, a::END_POINT).and_then(|v| v.xy()) { vec![e] } else { self.points_from_data(at, data) };
                for (x, y) in pts {
                    if rel {
                        let (ux, uy) = self.user_cursor();
                        self.line_to(ux + x, uy + y);
                    } else {
                        self.line_to(x, y);
                    }
                }
            }
            0xA0 | 0xA1 => {
                if op == 0xA0 {
                    self.path.clear();
                }
                if let Some(b) = get(at, a::BOUNDING_BOX).and_then(|v| v.bx()) {
                    self.rect_path(b);
                }
                if op == 0xA0 {
                    self.paint_current();
                }
            }
            0xA2 | 0xA3 => {
                if op == 0xA2 {
                    self.path.clear();
                }
                if let Some(b) = get(at, a::BOUNDING_BOX).and_then(|v| v.bx()) {
                    let (ew, eh) = get(at, a::ELLIPSE_DIMENSION).and_then(|v| v.xy()).unwrap_or((0.0, 0.0));
                    self.round_rect_path(b, ew, eh);
                }
                if op == 0xA2 {
                    self.paint_current();
                }
            }
            0xA8 | 0xA9 => self.text(at, op == 0xA9),
            0xB0 | 0xB3 => self.begin_image(at, op == 0xB3),
            0xB1 | 0xB4 => self.read_image(at, data),
            0xB2 | 0xB5 => self.end_image(),
            0xB6 | 0xB8 => {}
            0xB9 => self.doc.warn("ScanLineRel is not fully supported"),
            0x4A | 0x4B | 0x58 | 0x6D | 0x7E | 0x7F | 0x81 | 0x92 | 0x94 | 0x56 | 0x4C | 0x82 | 0x83 | 0xBA => {}
            _ => {
                self.doc.warn(format!("Unsupported PCL XL operator {op:#04X}"));
            }
        }
    }

    fn begin_page(&mut self, at: &Attrs) {
        self.finish_page_if_open();
        let orient = get(at, a::ORIENTATION).and_then(|v| v.num()).unwrap_or(0.0) as i64;
        let mut info = PageInfo { orientation: enum_name(a::ORIENTATION, orient as f64).unwrap_or("?").to_string(), ..Default::default() };
        let (mut mw, mut mh) = (612.0, 792.0);
        match get(at, a::MEDIA_SIZE) {
            Some(Val::Scalar(v, _)) => {
                let (w, h, n) = media_size_enum(*v as i64);
                mw = w;
                mh = h;
                info.media = n.to_string();
            }
            Some(v @ Val::Array(..)) => {
                let s = v.string();
                if let Some((w, h)) = media_size_name(&s) {
                    mw = w;
                    mh = h;
                } else {
                    self.doc.warn(format!("Unknown media size name {s}"));
                }
                info.media = s;
            }
            _ => {}
        }
        if let Some((w, h)) = get(at, a::CUSTOM_MEDIA_SIZE).and_then(|v| v.xy()) {
            let u = get(at, a::CUSTOM_MEDIA_SIZE_UNITS).and_then(|v| v.num()).unwrap_or(0.0) as i64;
            let f = match u {
                1 => 72.0 / 25.4,
                2 => 72.0 / 254.0,
                _ => 72.0,
            };
            mw = w * f;
            mh = h * f;
            info.media = format!("Custom {w} × {h} {}", enum_name(a::MEASURE, u as f64).unwrap_or(""));
        }
        if info.media.is_empty() {
            info.media = "Letter (default)".into();
        }
        if let Some(s) = get(at, a::MEDIA_SOURCE) {
            info.source = match s {
                Val::Scalar(v, _) => enum_name(a::MEDIA_SOURCE, *v).map(|x| x.to_string()).unwrap_or_else(|| format!("Tray {v}")),
                v => v.string(),
            };
        }
        if let Some(s) = get(at, a::MEDIA_TYPE) {
            info.mtype = s.string();
        }
        if let Some(s) = get(at, a::MEDIA_DESTINATION) {
            info.destination = match s {
                Val::Scalar(v, _) => enum_name(a::MEDIA_DESTINATION, *v).map(|x| x.to_string()).unwrap_or_else(|| format!("Bin {v}")),
                v => v.string(),
            };
        }
        if let Some(v) = get(at, a::DUPLEX_PAGE_MODE).and_then(|v| v.num()) {
            let side = get(at, a::DUPLEX_PAGE_SIDE).and_then(|v| v.num()).and_then(|s| enum_name(a::DUPLEX_PAGE_SIDE, s)).unwrap_or("");
            info.duplex = format!("{} {}", enum_name(a::DUPLEX_PAGE_MODE, v).unwrap_or("?"), side);
        } else if get(at, a::SIMPLEX_PAGE_MODE).is_some() {
            info.duplex = "Simplex".into();
        }
        if let Some(c) = get(at, a::PAGE_COPIES).and_then(|v| v.num()) {
            info.copies = format!("{c}");
        }
        self.cur_page_info = info;

        let landscape = orient == 1 || orient == 3;
        let (pw, ph) = if landscape { (mh, mw) } else { (mw, mh) };
        let s = Matrix::scale(72.0 / self.units.0, 72.0 / self.units.1);
        let base = if orient == 2 || orient == 3 { s.then(&Matrix::new(-1.0, 0.0, 0.0, -1.0, pw, ph)) } else { s };
        self.default_ctm = base;
        let font = self.gs.font.clone();
        self.gs = Gs::new(base);
        self.gs.font = font;
        self.gs_stack.clear();
        self.path.clear();
        self.cursor = (0.0, 0.0);
        self.page = Some(Page::new(pw as f32, ph as f32));
    }

    // ---- fonts ----

    fn define_font(&mut self, name: Vec<u8>, hdr: Vec<u8>) {
        if hdr.len() < 8 {
            self.doc.warn("Font header too short");
            return;
        }
        let symset = u16::from_be_bytes([hdr[2], hdr[3]]) as u32;
        let tech = hdr[4];
        // segments: try big-endian first, then little-endian
        let segs = parse_segments(&hdr, false).or_else(|| parse_segments(&hdr, true)).unwrap_or_default();
        let kind = if tech == 254 {
            let mut res = (300.0, 300.0);
            if let Some(br) = segs.get(&*b"BR") {
                if br.len() >= 4 {
                    res = (u16::from_be_bytes([br[0], br[1]]) as f64, u16::from_be_bytes([br[2], br[3]]) as f64);
                    if res.0 <= 0.0 || res.0 > 10000.0 {
                        res = (u16::from_le_bytes([br[0], br[1]]) as f64, u16::from_le_bytes([br[2], br[3]]) as f64);
                    }
                }
            }
            XGlyphs::Bitmap { res, chars: HashMap::new() }
        } else {
            let sfnt = segs.get(&*b"GT").map(|gt| SfntInfo::parse(gt)).unwrap_or_else(|| SfntInfo { upem: 2048.0, ..Default::default() });
            XGlyphs::TrueType { sfnt, by_code: HashMap::new(), by_gid: HashMap::new() }
        };
        self.fonts.insert(name, XFont { kind, symset, header_len: hdr.len(), ident: None, ident_tried: 0 });
    }

    fn define_char(&mut self, name: &[u8], code: u32, d: &[u8]) {
        let Some(f) = self.fonts.get_mut(name) else { return };
        match &mut f.kind {
            XGlyphs::Bitmap { chars, .. } => {
                if d.len() < 10 {
                    return;
                }
                let be = |o: usize| u16::from_be_bytes([d[o], d[o + 1]]);
                let le = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
                let mut w = be(6) as u32;
                let mut h = be(8) as u32;
                let mut left = be(2) as i16 as i32;
                let mut top = be(4) as i16 as i32;
                let need = |w: u32, h: u32| (w as usize).div_ceil(8) * h as usize;
                if need(w, h) + 10 > d.len() || w > 4096 || h > 4096 {
                    w = le(6) as u32;
                    h = le(8) as u32;
                    left = le(2) as i16 as i32;
                    top = le(4) as i16 as i32;
                }
                if d[1] != 0 {
                    // compressed bitmap chars (class 1) not supported
                    return;
                }
                let n = need(w, h);
                if 10 + n <= d.len() {
                    chars.insert(code, BitmapChar { left, top, w, h, bits: d[10..10 + n].to_vec() });
                }
            }
            XGlyphs::TrueType { by_code, by_gid, .. } => {
                if d.len() < 6 {
                    return;
                }
                let class = d[1];
                let be = |o: usize| u16::from_be_bytes([d[o], d[o + 1]]);
                let le16 = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
                // Decide byte order using the char data size field.
                let size_be = be(2) as usize;
                let size_le = le16(2) as usize;
                let use_le = !(size_be + 4 == d.len() || size_be + 2 == d.len() || size_be == d.len()) && (size_le + 4 == d.len() || size_le + 2 == d.len() || size_le == d.len());
                let r = |o: usize| if use_le { le16(o) } else { be(o) };
                // Candidate layouts: (glyph id offset, advance offset, glyph data offset)
                let layouts: &[(usize, Option<usize>, usize)] = match class {
                    0 => &[(4, None, 6), (8, Some(6), 10)],
                    1 => &[(8, Some(6), 10), (4, None, 6)],
                    2 => &[(10, Some(6), 12), (8, Some(6), 10), (4, None, 6)],
                    _ => &[(4, None, 6), (8, Some(6), 10)],
                };
                let sane = |off: usize| -> bool {
                    if off > d.len() {
                        return false;
                    }
                    let g = &d[off..];
                    if g.is_empty() {
                        return true;
                    }
                    if g.len() < 10 {
                        return false;
                    }
                    let nc = i16::from_be_bytes([g[0], g[1]]);
                    let b: Vec<i16> = (0..4).map(|k| i16::from_be_bytes([g[2 + 2 * k], g[3 + 2 * k]])).collect();
                    (-1..=2000).contains(&nc) && b[0] <= b[2] && b[1] <= b[3] && b.iter().all(|v| v.abs() < 16384)
                };
                let chosen = layouts.iter().find(|l| sane(l.2)).unwrap_or(&layouts[0]);
                let (gid, adv, gstart) = (r(chosen.0), chosen.1.map(|o| r(o) as f64), chosen.2);
                if gstart <= d.len() {
                    by_gid.insert(gid, d[gstart..].to_vec());
                    by_code.insert(code, (gid, adv));
                }
            }
        }
    }

    fn code_to_char(symset: u32, code: u32) -> char {
        if code > 255 {
            return char::from_u32(code).unwrap_or(' ');
        }
        match symset {
            277 => fonts::roman8(code as u8),
            14 | 18 => code as u8 as char,
            _ => fonts::cp1252(code as u8),
        }
    }

    fn text(&mut self, at: &Attrs, as_path: bool) {
        let codes: Vec<u32> = get(at, a::TEXT_DATA).and_then(|v| v.arr()).unwrap_or_default().iter().map(|v| *v as u32).collect();
        let xsp = get(at, a::X_SPACING_DATA).and_then(|v| v.arr());
        let ysp = get(at, a::Y_SPACING_DATA).and_then(|v| v.arr());
        let Some(sel) = self.gs.font.clone() else {
            self.doc.warn("Text drawn with no font selected");
            return;
        };
        let ctm = self.gs.ctm;
        let (mut ux, mut uy) = self.user_cursor();
        let gs = &self.gs;
        let size = sel.size;
        let (csx, csy) = gs.char_scale;
        let glyph_m = |upem: f64, x: f64, y: f64, gs: &Gs| -> Matrix {
            let s = size / upem;
            Matrix::scale(s * csx, -s * csy)
                .then(&Matrix::new(1.0, gs.char_shear.1, gs.char_shear.0, 1.0, 0.0, 0.0))
                .then(&Matrix::rotate_deg(gs.char_angle))
                .then(&Matrix::translate(x, y))
                .then(&ctm)
        };
        let mut outline = PathAcc::new();
        let mut images: Vec<Item> = Vec::new();
        let color = match &self.gs.brush {
            XPaint::Solid(c) => *c,
            _ => Rgb::BLACK,
        };
        let downloaded = self.fonts.contains_key(&sel.name);
        if let Some(f) = self.fonts.get_mut(&sel.name) {
            if let XGlyphs::TrueType { by_gid, .. } = &f.kind {
                let n = by_gid.len();
                if f.ident.is_none() && n >= 3 && (f.ident_tried == 0 || n >= f.ident_tried * 2) && f.ident_tried < 1000 {
                    f.ident_tried = n.max(1);
                    let samples: Vec<(u16, Vec<u8>)> = by_gid.iter().map(|(g, d)| (*g, d.clone())).collect();
                    f.ident = fonts::identify_subset(&samples);
                }
            }
        }
        let mut chars_out: Vec<(char, (f64, f64), (f64, f64))> = Vec::new();
        let up = ctm.apply_delta(0.0, -size * csy);
        let sysfont = if downloaded {
            None
        } else {
            let name = sel.name.clone();
            Some(self.sys_fonts.entry(name.clone()).or_insert_with(|| fonts::system_font(&String::from_utf8_lossy(&name))).clone())
        };
        for (i, &code) in codes.iter().enumerate() {
            let mut adv: Option<f64> = None;
            let start = ctm.apply(ux, uy);
            let mut ch_out = '\u{FFFD}';
            if let Some(f) = self.fonts.get(&sel.name) {
                ch_out = match &f.kind {
                    XGlyphs::TrueType { by_code, .. } => by_code
                        .get(&code)
                        .and_then(|(gid, _)| f.ident.as_ref().and_then(|id| id.map.get(gid).copied()))
                        .unwrap_or_else(|| if f.symset == 0 || f.ident.is_some() { '\u{FFFD}' } else { Self::code_to_char(f.symset, code) }),
                    XGlyphs::Bitmap { .. } => Self::code_to_char(f.symset, code),
                };
                match &f.kind {
                    XGlyphs::TrueType { sfnt, by_code, by_gid } => {
                        if let Some((gid, cadv)) = by_code.get(&code) {
                            if let Some(gd) = by_gid.get(gid) {
                                let m = glyph_m(sfnt.upem, ux, uy, &self.gs);
                                let lookup = |g: u16| by_gid.get(&g).cloned().or_else(|| sfnt.glyph_data(g));
                                fonts::glyf_outline(gd, &m, &mut outline, &lookup, 0);
                            }
                            let a_units = cadv.or_else(|| sfnt.advance(*gid));
                            adv = a_units.map(|a| a * size / sfnt.upem * csx);
                        }
                    }
                    XGlyphs::Bitmap { res, chars } => {
                        if let Some(ch) = chars.get(&code) {
                            let key = (sel.name.clone(), code, pack_rgb(color));
                            let img = self.glyph_cache.entry(key).or_insert_with(|| Arc::new(bitmap_glyph(ch, color))).clone();
                            let px = self.units.0 / res.0;
                            let py = self.units.1 / res.1;
                            let m = Matrix::scale(px, py).then(&Matrix::translate(ux + ch.left as f64 * px, uy - ch.top as f64 * py)).then(&ctm);
                            images.push(Item::Image { image: img, transform: m.to_ts(), clip: self.gs.clip.clone() });
                        }
                    }
                }
            } else if let Some(sf) = &sysfont {
                let ch = Self::code_to_char(sel.symset, code);
                ch_out = ch;
                if let Some(g) = sf.glyph_for_char(ch) {
                    let m = glyph_m(sf.upem, ux, uy, &self.gs);
                    sf.outline(g, &m, &mut outline);
                    adv = Some(sf.advance(g) * size / sf.upem * csx);
                } else if ch != ' ' {
                    adv = Some(size * 0.5 * csx);
                } else {
                    adv = Some(size * 0.25 * csx);
                }
            }
            let dx = xsp.as_ref().and_then(|x| x.get(i).copied()).or(adv).unwrap_or(0.0);
            let dy = ysp.as_ref().and_then(|y| y.get(i).copied()).unwrap_or(0.0);
            if xsp.is_some() || ysp.is_some() {
                ux += dx;
                uy += dy;
            } else {
                let r = self.gs.char_angle.to_radians();
                ux += dx * r.cos();
                uy += dx * r.sin();
            }
            chars_out.push((ch_out, start, ctm.apply(ux, uy)));
        }
        self.cursor = ctm.apply(ux, uy);
        if !as_path {
            if let Some(pg) = self.page.as_mut() {
                for (c, a, b) in chars_out {
                    pg.text_char(c, a, b, up);
                }
                pg.text_break();
                pg.cur_kind = Kind::Text;
            }
        }
        if as_path {
            self.path.append(&outline);
            return;
        }
        if !outline.is_empty() {
            self.fill_path(&outline);
            if self.gs.char_bold > 0.0 {
                if let Some(brush) = self.brush_of(&self.gs.brush.clone()) {
                    if let (Some(inv), true) = (ctm.invert(), true) {
                        if let Some(path) = outline.to_path(&inv) {
                            let style = StrokeStyle { width: (self.gs.char_bold * size) as f32, cap: LineCap::Round, join: LineJoin::Round, miter: 4.0, dash: None };
                            self.push_item(Item::Stroke { path, transform: ctm.to_ts(), brush, style, clip: self.gs.clip.clone() });
                        }
                    }
                }
            }
        }
        for it in images {
            self.push_item(it);
        }
        if let Some(pg) = self.page.as_mut() {
            pg.cur_kind = Kind::Vector;
        }
    }

    // ---- images ----

    fn begin_image(&mut self, at: &Attrs, pattern: bool) {
        let mapping = get(at, a::COLOR_MAPPING).and_then(|v| v.num()).unwrap_or(0.0) as u8;
        let depth = match get(at, a::COLOR_DEPTH).and_then(|v| v.num()).unwrap_or(2.0) as u8 {
            0 => 1,
            1 => 4,
            _ => 8,
        };
        let sw = get(at, a::SOURCE_WIDTH).and_then(|v| v.num()).unwrap_or(0.0) as u32;
        let sh = get(at, a::SOURCE_HEIGHT).and_then(|v| v.num()).unwrap_or(0.0) as u32;
        let (dw, dh) = get(at, a::DESTINATION_SIZE)
            .and_then(|v| v.xy())
            .or_else(|| get(at, a::DESTINATION_BOX).and_then(|v| v.bx()).map(|b| (b[2] - b[0], b[3] - b[1])))
            .unwrap_or((sw as f64, sh as f64));
        let origin = self.user_cursor();
        let pattern_id = if pattern { Some(get(at, a::PATTERN_DEFINE_ID).and_then(|v| v.num()).unwrap_or(0.0) as i64) } else { None };
        let key = format!("{}{} {}-bit {}", if pattern { "Pattern " } else { "" }, if mapping == 1 { "indexed" } else if self.gs.color_space == 1 { "gray" } else { "RGB" }, depth, "");
        *self.image_stats.entry(key.trim().to_string()).or_insert(0) += 1;
        self.image = Some(ImageState { mapping, depth, sw, sh, dw, dh, origin, pattern_id, rows: Vec::new(), seed: Vec::new() });
    }

    fn read_image(&mut self, at: &Attrs, data: Option<&[u8]>) {
        let Some(d) = data else { return };
        let start = get(at, a::START_LINE).and_then(|v| v.num()).unwrap_or(0.0) as u32;
        let bh = get(at, a::BLOCK_HEIGHT).and_then(|v| v.num()).unwrap_or(1.0) as u32;
        let cm = get(at, a::COMPRESS_MODE).and_then(|v| v.num()).unwrap_or(0.0) as u8;
        let pad = get(at, a::PAD_BYTES_MULTIPLE).and_then(|v| v.num()).unwrap_or(4.0).max(1.0) as usize;
        let Some(img) = self.image.as_mut() else { return };
        let comps = if img.mapping == 1 { 1 } else if self.gs.color_space == 1 || self.gs.color_space == 0 { 1 } else { 3 };
        let row_bytes = (img.sw as usize * img.depth as usize * comps).div_ceil(8);
        let padded = row_bytes.div_ceil(pad) * pad;
        let le = self.le;
        let mut name = match cm {
            1 => "RLE",
            2 => "JPEG",
            3 => "DeltaRow",
            _ => "Uncompressed",
        };
        let rgba: Option<(u32, u32, Vec<u8>)> = match cm {
            0 | 1 => {
                let raw = if cm == 1 { packbits(d, padded * bh as usize) } else { d.to_vec() };
                Some((img.sw, bh, unpack_rows(&raw, img.sw, bh, padded, img.depth, comps, img.mapping, &self.gs, None)))
            }
            3 => {
                let mut out = Vec::with_capacity(row_bytes * bh as usize);
                if img.seed.len() != row_bytes {
                    img.seed = vec![0; row_bytes];
                }
                let mut p = 0;
                for _ in 0..bh {
                    let Some(cnt) = rd_u16(d, p, le) else { break };
                    p += 2;
                    let end = (p + cnt as usize).min(d.len());
                    delta_row(&d[p..end], &mut img.seed);
                    out.extend_from_slice(&img.seed);
                    p = end;
                }
                Some((img.sw, bh, unpack_rows(&out, img.sw, bh, row_bytes, img.depth, comps, img.mapping, &self.gs, None)))
            }
            2 => {
                let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(d));
                match dec.decode() {
                    Ok(px) => {
                        let info = dec.info().unwrap();
                        let (w, h) = (info.width as u32, info.height as u32);
                        let mut out = Vec::with_capacity((w * h * 4) as usize);
                        match info.pixel_format {
                            jpeg_decoder::PixelFormat::L8 => px.iter().for_each(|&g| out.extend_from_slice(&[g, g, g, 255])),
                            jpeg_decoder::PixelFormat::RGB24 => px.chunks_exact(3).for_each(|c| out.extend_from_slice(&[c[0], c[1], c[2], 255])),
                            jpeg_decoder::PixelFormat::CMYK32 => px.chunks_exact(4).for_each(|c| {
                                let k = 255 - c[3] as u32;
                                out.extend_from_slice(&[((255 - c[0] as u32) * k / 255) as u8, ((255 - c[1] as u32) * k / 255) as u8, ((255 - c[2] as u32) * k / 255) as u8, 255])
                            }),
                            _ => {
                                name = "JPEG (unsupported format)";
                                out.clear();
                            }
                        }
                        if out.is_empty() { None } else { Some((w, h, out)) }
                    }
                    Err(e) => {
                        self.doc.warn(format!("JPEG decode error: {e}"));
                        None
                    }
                }
            }
            _ => {
                self.doc.warn(format!("Unknown image compression {cm}"));
                None
            }
        };
        *self.image_stats.entry(format!("Image blocks: {name}")).or_insert(0) += 1;
        let Some((w, h, mut px)) = rgba else { return };
        let Some(img) = self.image.as_mut() else { return };
        if self.gs.src_tx == 1 {
            for c in px.chunks_exact_mut(4) {
                if c[0] == 255 && c[1] == 255 && c[2] == 255 {
                    c.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        if img.pattern_id.is_some() {
            img.rows.extend_from_slice(&px);
            return;
        }
        if w == 0 || h == 0 || px.len() < (w * h * 4) as usize {
            return;
        }
        let sx = img.dw / img.sw.max(1) as f64;
        let sy = img.dh / img.sh.max(1) as f64;
        // JPEG blocks may have their own height
        let m = Matrix::scale(sx * img.sw as f64 / w as f64, sy).then(&Matrix::translate(img.origin.0, img.origin.1 + start as f64 * sy)).then(&self.gs.ctm);
        let (image, m) = RasterImage::fitted(w, h, px, &m);
        let clip = self.gs.clip.clone();
        self.push_item(Item::Image { image: Arc::new(image), transform: m.to_ts(), clip });
    }

    fn end_image(&mut self) {
        if let Some(img) = self.image.take() {
            if let Some(id) = img.pattern_id {
                let h = (img.rows.len() / 4 / img.sw.max(1) as usize) as u32;
                if img.sw > 0 && h > 0 {
                    let mut rgba = img.rows;
                    rgba.truncate((img.sw * h * 4) as usize);
                    self.patterns.insert(id, PatternDef { image: Arc::new(RasterImage::new(img.sw, h, rgba)), dw: img.dw, dh: img.dh * h as f64 / img.sh.max(1) as f64 });
                }
            }
        }
    }

    // ---- info ----

    pub fn finish(self) {
        let mut s = InfoSection::new("PCL XL stream");
        s.add("Stream header", self.header.clone());
        let parts: Vec<&str> = self.header.split(';').collect();
        if parts.len() >= 3 {
            s.add("Binding", if self.header.starts_with(')') { "Binary, little-endian" } else if self.header.starts_with('(') { "Binary, big-endian" } else { "ASCII" });
            s.add("Protocol class", format!("{}.{}", parts[1].trim(), parts[2].trim()));
            if parts.len() > 3 {
                s.add("Creator comment", parts[3..].join(";").trim().to_string());
            }
        }
        self.doc.info.push(s);
        self.doc.info.push(self.session);

        let mut pg = InfoSection::new("Pages (PCL XL)");
        for (i, p) in self.page_infos.iter().enumerate() {
            let mut v = format!("{} · {}", p.media, p.orientation);
            if !p.source.is_empty() {
                v.push_str(&format!(" · source {}", p.source));
            }
            if !p.mtype.is_empty() {
                v.push_str(&format!(" · type {}", p.mtype));
            }
            if !p.destination.is_empty() {
                v.push_str(&format!(" · output {}", p.destination));
            }
            if !p.duplex.is_empty() {
                v.push_str(&format!(" · {}", p.duplex));
            }
            if !p.copies.is_empty() {
                v.push_str(&format!(" · copies {}", p.copies));
            }
            pg.add(format!("Page {}", i + 1), v);
        }
        self.doc.info.push(pg);

        let mut f = InfoSection::new("Fonts (PCL XL)");
        for (name, font) in &self.fonts {
            let n = String::from_utf8_lossy(name).trim().to_string();
            let desc = match &font.kind {
                XGlyphs::TrueType { by_code, sfnt, .. } => format!(
                    "Downloaded TrueType, {} glyphs, {} units/em, symbol set {}, header {} bytes{}",
                    by_code.len(),
                    sfnt.upem,
                    symset_name(font.symset),
                    font.header_len,
                    font.ident.as_ref().map(|i| format!(" — glyphs match installed font \"{}\"", i.name)).unwrap_or_default()
                ),
                XGlyphs::Bitmap { chars, res } => format!("Downloaded bitmap, {} chars, {}×{} dpi, symbol set {}", chars.len(), res.0, res.1, symset_name(font.symset)),
            };
            f.add(n, desc);
        }
        for (k, v) in &self.font_uses {
            f.add(format!("Used: {k}"), format!("{v} SetFont call(s)"));
        }
        for (k, sf) in &self.sys_fonts {
            f.add(format!("Substitute for {}", String::from_utf8_lossy(k).trim()), sf.file.clone());
        }
        self.doc.info.push(f);

        if !self.image_stats.is_empty() {
            let mut im = InfoSection::new("Raster images (PCL XL)");
            for (k, v) in &self.image_stats {
                im.add(k.clone(), v.to_string());
            }
            self.doc.info.push(im);
        }
        if !self.comments.is_empty() {
            let mut c = InfoSection::new("Embedded comments");
            for (i, cm) in self.comments.iter().enumerate() {
                c.add(format!("{}", i + 1), cm.clone());
            }
            self.doc.info.push(c);
        }
        if !self.streams.is_empty() {
            let mut c = InfoSection::new("User-defined streams");
            for (k, v) in &self.streams {
                c.add(String::from_utf8_lossy(k).to_string(), fmt_bytes(v.len()));
            }
            self.doc.info.push(c);
        }
        let mut oc = InfoSection::new("Operator statistics");
        let total: usize = self.op_counts.values().sum();
        oc.add("Total operators", total.to_string());
        let mut v: Vec<_> = self.op_counts.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in v {
            oc.add(k.to_string(), n.to_string());
        }
        self.doc.info.push(oc);
    }
}

fn symset_name(v: u32) -> String {
    if v == 0 {
        return "0".into();
    }
    let num = v / 32;
    let letter = (v % 32 + 64) as u8 as char;
    let known = match v {
        277 => " (Roman-8)",
        629 => " (Windows Latin 1)",
        14 => " (ISO Latin 1)",
        21 => " (ASCII)",
        341 => " (PC-8)",
        621 => " (Symbol)",
        18540 => " (Wingdings)",
        590 => " (Unicode)",
        _ => "",
    };
    format!("{num}{letter}{known}")
}

fn parse_segments(hdr: &[u8], le: bool) -> Option<HashMap<[u8; 2], Vec<u8>>> {
    let mut map = HashMap::new();
    let mut p = 8;
    let tech = hdr[4];
    while p + 2 <= hdr.len() {
        let id = [hdr[p], hdr[p + 1]];
        if id == [0xFF, 0xFF] {
            return Some(map);
        }
        if !(id[0].is_ascii_uppercase() && (id[1].is_ascii_uppercase() || id[1].is_ascii_digit())) {
            return if map.is_empty() { None } else { Some(map) };
        }
        let (len, hl) = if tech == 254 {
            let v = if le { u16::from_le_bytes([*hdr.get(p + 2)?, *hdr.get(p + 3)?]) } else { u16::from_be_bytes([*hdr.get(p + 2)?, *hdr.get(p + 3)?]) };
            (v as usize, 4)
        } else {
            let b = [*hdr.get(p + 2)?, *hdr.get(p + 3)?, *hdr.get(p + 4)?, *hdr.get(p + 5)?];
            (if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) } as usize, 6)
        };
        let s = p + hl;
        if s + len > hdr.len() {
            return None;
        }
        map.insert(id, hdr[s..s + len].to_vec());
        p = s + len;
    }
    Some(map)
}

fn pack_rgb(c: Rgb) -> u32 {
    ((c.0 * 255.0) as u32) << 16 | ((c.1 * 255.0) as u32) << 8 | (c.2 * 255.0) as u32
}

fn bitmap_glyph(ch: &BitmapChar, c: Rgb) -> RasterImage {
    let stride = (ch.w as usize).div_ceil(8);
    let mut rgba = vec![0u8; (ch.w * ch.h * 4) as usize];
    let (r, g, b) = ((c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8);
    for y in 0..ch.h as usize {
        for x in 0..ch.w as usize {
            if ch.bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0 {
                let o = (y * ch.w as usize + x) * 4;
                rgba[o..o + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    RasterImage::new(ch.w.max(1), ch.h.max(1), if ch.w == 0 || ch.h == 0 { vec![0; 4] } else { rgba })
}

pub fn packbits(d: &[u8], expect: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(expect);
    let mut p = 0;
    while p < d.len() {
        let n = d[p] as i8;
        p += 1;
        if n >= 0 {
            let c = n as usize + 1;
            let e = (p + c).min(d.len());
            out.extend_from_slice(&d[p..e]);
            p = e;
        } else if n != -128 {
            let c = 1 - n as isize;
            if p < d.len() {
                out.extend(std::iter::repeat(d[p]).take(c as usize));
            }
            p += 1;
        }
    }
    out
}

pub fn delta_row(cmds: &[u8], seed: &mut [u8]) {
    let mut p = 0;
    let mut pos = 0usize;
    while p < cmds.len() {
        let c = cmds[p];
        p += 1;
        let count = (c >> 5) as usize + 1;
        let mut off = (c & 0x1F) as usize;
        if off == 31 {
            while p < cmds.len() {
                let b = cmds[p] as usize;
                p += 1;
                off += b;
                if b != 255 {
                    break;
                }
            }
        }
        pos += off;
        for _ in 0..count {
            if p >= cmds.len() {
                break;
            }
            if pos < seed.len() {
                seed[pos] = cmds[p];
            }
            pos += 1;
            p += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn unpack_rows(raw: &[u8], w: u32, h: u32, stride: usize, depth: u8, comps: usize, mapping: u8, gs: &Gs, _x: Option<()>) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let maxv = ((1u32 << depth) - 1) as f32;
    let pal = gs.palette.as_ref();
    let rgb_pal = gs.color_space == 2 || gs.color_space == 6;
    for y in 0..h as usize {
        let row = raw.get(y * stride..).unwrap_or(&[]);
        let sample = |i: usize| -> u32 {
            let bit = i * depth as usize;
            let byte = row.get(bit / 8).copied().unwrap_or(0) as u32;
            match depth {
                8 => byte,
                4 => (byte >> (4 - bit % 8)) & 0xF,
                1 => (byte >> (7 - bit % 8)) & 1,
                _ => byte,
            }
        };
        for x in 0..w as usize {
            if mapping == 1 {
                let idx = sample(x) as usize;
                let c = match pal {
                    Some(p) if rgb_pal && idx * 3 + 2 < p.len() => [p[idx * 3], p[idx * 3 + 1], p[idx * 3 + 2]],
                    Some(p) if !rgb_pal && idx < p.len() => [p[idx]; 3],
                    _ => {
                        let g = (idx as f32 / maxv * 255.0) as u8;
                        [g; 3]
                    }
                };
                out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            } else if comps == 3 {
                let r = (sample(x * 3) as f32 / maxv * 255.0) as u8;
                let g = (sample(x * 3 + 1) as f32 / maxv * 255.0) as u8;
                let b = (sample(x * 3 + 2) as f32 / maxv * 255.0) as u8;
                out.extend_from_slice(&[r, g, b, 255]);
            } else {
                let g = (sample(x) as f32 / maxv * 255.0) as u8;
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
    }
    out
}
