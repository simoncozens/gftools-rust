//! A font's own TrueType hinting, kept as a text file beside its sources.
//!
//! A source format cannot carry a compiled font's TrueType hinting: its programs
//! address the glyphs' points by index, and the device metrics tables (hdmx, LTSH,
//! VDMX) have no source form. A hinting file holds them and is applied to the
//! compiled font, which must have the same points. The format is the one gftools'
//! `gftools.builder.apply_hinting` reads and writes:
//!
//! - `fpgm`, `prep`: programs in fontTools assembly
//! - `cvt`: control values
//! - `gasp`: `{ppem: behavior}`
//! - `maxp`: the TrueType limits (maxZones ... maxSizeOfInstructions)
//! - `head_flag_3`: whether ppem is rounded to an integer
//! - `hdmx`, `LTSH`, `VDMX`: device metrics, when the font has them
//! - `glyphs`: `{glyph: {"assembly": program, "id": digest}}`, the id being the
//!   SHA-256 of the glyph's point text (see [`point_text`])
//! - `components`: each composite's component flags that affect hinting
//! - `contour_starts`: `{glyph: [[x, y, on], ...]}`, each contour's first point,
//!   which a source does not fix (a closed path cannot start off-curve, and
//!   converters and compilers choose their own start)
//! - `transforms`: component matrices whose F2Dot14 values a source does not keep
//!   at four decimals
//! - `lsb_shifts`: `{glyph: {"xmin": x, "lsb": lsb, "bbox": [xMin, yMin, xMax,
//!   yMax]}}`, a simple glyph whose left side bearing differs from its recorded
//!   xMin (a converter may draw it shifted by the difference): its points are
//!   moved so that the leftmost is at `x` again, and its recorded box and side
//!   bearing restored, which a TrueType rasterizer places it by
//! - `boxes`: `{composite: {"bbox": [xMin, yMin, xMax, yMax], "lsb": lsb}}`, a
//!   composite whose recorded box differs from the one its components give (a
//!   stale box, which a TrueType rasterizer places the glyph by): restored
//! - `head_flags_2_4`: head flags bits 2 and 4 (instructions may depend on point
//!   size, may alter advance width), as the release sets them
//! - `offsets`: `{composite: [[x, y], ...]}`, the component offsets of a
//!   composite that places a shifted glyph, or is shifted itself
//! - `glyph_order`: the font's glyph names in order; a name the compiled font
//!   does not have (a release with duplicate or no glyph names) is matched by
//!   its index, when both fonts have as many glyphs
//! - `midpoints`: `{glyph: {"points": [index, ...], "ends": [...]}}`, on-curve
//!   points exactly halfway between two off-curves, which a compiler may drop as
//!   implied, with the glyph's contour ends
//!
//! Applying refuses every glyph whose points do not match the file (a program
//! would address other points), and changes nothing in that case.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use skrifa::{
    GlyphId, GlyphNames, Tag,
    raw::{FontRef, TableProvider, tables::glyf::CurvePoint},
};
use write_fonts::{
    FontBuilder,
    from_obj::ToOwnedTable,
    tables::{
        glyf::{Anchor, Contour, GlyfLocaBuilder, Glyph, Transform},
        head::Head,
        maxp::Maxp,
    },
    types::F2Dot14,
};

use crate::GftoolsError;

const FORMAT_VERSION: &str = "1";
// ROUND_XY_TO_GRID, USE_MY_METRICS, OVERLAP_COMPOUND, SCALED_COMPONENT_OFFSET,
// UNSCALED_COMPONENT_OFFSET
const ROUND_XY_TO_GRID: u16 = 0x0004;
const USE_MY_METRICS: u16 = 0x0200;
const OVERLAP_COMPOUND: u16 = 0x0400;
const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;
const F2DOT14_UNIT: f64 = 1.0 / 16384.0;

/// One glyph's program and the digest of the points it was written for.
#[derive(Debug, Deserialize)]
pub struct GlyphProgram {
    pub assembly: String,
    pub id: String,
}

/// Midpoint on-curve points of one glyph, by release index, and its contour ends.
#[derive(Debug, Deserialize)]
pub struct Midpoints {
    pub points: Vec<usize>,
    pub ends: Vec<usize>,
}

/// A glyph whose recorded xMin differs from its side bearing (see `lsb_shifts`).
#[derive(Debug, Deserialize)]
pub struct LsbShift {
    pub xmin: i16,
    pub lsb: i16,
    pub bbox: [i16; 4],
}

/// A composite's recorded box and side bearing (see `boxes`).
#[derive(Debug, Deserialize)]
pub struct RecordedBox {
    pub bbox: [i16; 4],
    pub lsb: i16,
}

/// A VDMX aspect ratio range, pointing at one of the groups.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatRange {
    pub b_char_set: u8,
    pub x_ratio: u8,
    pub y_start_ratio: u8,
    pub y_end_ratio: u8,
    pub group_index: usize,
}

/// The VDMX table.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vdmx {
    pub version: u16,
    pub rat_ranges: Vec<RatRange>,
    /// `{yPelHeight: [yMax, yMin]}` per group
    pub groups: Vec<BTreeMap<String, (i16, i16)>>,
}

/// A hinting file.
#[derive(Debug, Deserialize)]
pub struct HintingFile {
    #[serde(rename = "formatVersion")]
    pub format_version: String,
    pub fpgm: Option<String>,
    pub prep: Option<String>,
    pub cvt: Option<Vec<i16>>,
    pub gasp: Option<BTreeMap<String, u16>>,
    pub maxp: BTreeMap<String, u16>,
    pub head_flag_3: bool,
    pub hdmx: Option<BTreeMap<String, BTreeMap<String, u8>>>,
    #[serde(rename = "LTSH")]
    pub ltsh: Option<BTreeMap<String, u8>>,
    #[serde(rename = "VDMX")]
    pub vdmx: Option<Vdmx>,
    pub glyphs: BTreeMap<String, GlyphProgram>,
    #[serde(default)]
    pub components: BTreeMap<String, Vec<u16>>,
    #[serde(default)]
    pub contour_starts: BTreeMap<String, Vec<(i16, i16, u8)>>,
    #[serde(default)]
    pub transforms: BTreeMap<String, Vec<[f64; 4]>>,
    #[serde(default)]
    pub midpoints: BTreeMap<String, Midpoints>,
    #[serde(default)]
    pub glyph_order: Vec<String>,
    #[serde(default)]
    pub lsb_shifts: BTreeMap<String, LsbShift>,
    #[serde(default)]
    pub offsets: BTreeMap<String, Vec<Option<(i16, i16)>>>,
    #[serde(default)]
    pub boxes: BTreeMap<String, RecordedBox>,
    #[serde(default)]
    pub head_flags_2_4: Option<u16>,
}

impl HintingFile {
    /// Read a hinting file.
    pub fn from_json(text: &str) -> Result<Self, GftoolsError> {
        let file: HintingFile = serde_json::from_str(text)
            .map_err(|e| GftoolsError::Misc(format!("hinting file: {e}")))?;
        if file.format_version != FORMAT_VERSION {
            return Err(GftoolsError::Misc(format!(
                "hinting file: unknown formatVersion {:?}",
                file.format_version
            )));
        }
        Ok(file)
    }
}

/// TrueType instructions: mnemonic, first opcode, number of flag bits (from
/// fontTools' ttProgram instruction table). PUSHB, PUSHW, NPUSHB and NPUSHW are
/// handled apart.
const INSTRUCTIONS: &[(&str, u8, u32)] = &[
    ("AA", 0x7F, 0),
    ("ABS", 0x64, 0),
    ("ADD", 0x60, 0),
    ("ALIGNPTS", 0x27, 0),
    ("ALIGNRP", 0x3C, 0),
    ("AND", 0x5A, 0),
    ("CALL", 0x2B, 0),
    ("CEILING", 0x67, 0),
    ("CINDEX", 0x25, 0),
    ("CLEAR", 0x22, 0),
    ("DEBUG", 0x4F, 0),
    ("DELTAC1", 0x73, 0),
    ("DELTAC2", 0x74, 0),
    ("DELTAC3", 0x75, 0),
    ("DELTAP1", 0x5D, 0),
    ("DELTAP2", 0x71, 0),
    ("DELTAP3", 0x72, 0),
    ("DEPTH", 0x24, 0),
    ("DIV", 0x62, 0),
    ("DUP", 0x20, 0),
    ("EIF", 0x59, 0),
    ("ELSE", 0x1B, 0),
    ("ENDF", 0x2D, 0),
    ("EQ", 0x54, 0),
    ("EVEN", 0x57, 0),
    ("FDEF", 0x2C, 0),
    ("FLIPOFF", 0x4E, 0),
    ("FLIPON", 0x4D, 0),
    ("FLIPPT", 0x80, 0),
    ("FLIPRGOFF", 0x82, 0),
    ("FLIPRGON", 0x81, 0),
    ("FLOOR", 0x66, 0),
    ("GC", 0x46, 1),
    ("GETINFO", 0x88, 0),
    ("GETVARIATION", 0x91, 0),
    ("GFV", 0x0D, 0),
    ("GPV", 0x0C, 0),
    ("GT", 0x52, 0),
    ("GTEQ", 0x53, 0),
    ("IDEF", 0x89, 0),
    ("IF", 0x58, 0),
    ("INSTCTRL", 0x8E, 0),
    ("IP", 0x39, 0),
    ("ISECT", 0x0F, 0),
    ("IUP", 0x30, 1),
    ("JMPR", 0x1C, 0),
    ("JROF", 0x79, 0),
    ("JROT", 0x78, 0),
    ("LOOPCALL", 0x2A, 0),
    ("LT", 0x50, 0),
    ("LTEQ", 0x51, 0),
    ("MAX", 0x8B, 0),
    ("MD", 0x49, 1),
    ("MDAP", 0x2E, 1),
    ("MDRP", 0xC0, 5),
    ("MIAP", 0x3E, 1),
    ("MIN", 0x8C, 0),
    ("MINDEX", 0x26, 0),
    ("MIRP", 0xE0, 5),
    ("MPPEM", 0x4B, 0),
    ("MPS", 0x4C, 0),
    ("MSIRP", 0x3A, 1),
    ("MUL", 0x63, 0),
    ("NEG", 0x65, 0),
    ("NEQ", 0x55, 0),
    ("NOT", 0x5C, 0),
    ("NROUND", 0x6C, 2),
    ("ODD", 0x56, 0),
    ("OR", 0x5B, 0),
    ("POP", 0x21, 0),
    ("RCVT", 0x45, 0),
    ("RDTG", 0x7D, 0),
    ("ROFF", 0x7A, 0),
    ("ROLL", 0x8A, 0),
    ("ROUND", 0x68, 2),
    ("RS", 0x43, 0),
    ("RTDG", 0x3D, 0),
    ("RTG", 0x18, 0),
    ("RTHG", 0x19, 0),
    ("RUTG", 0x7C, 0),
    ("S45ROUND", 0x77, 0),
    ("SANGW", 0x7E, 0),
    ("SCANCTRL", 0x85, 0),
    ("SCANTYPE", 0x8D, 0),
    ("SCFS", 0x48, 0),
    ("SCVTCI", 0x1D, 0),
    ("SDB", 0x5E, 0),
    ("SDPVTL", 0x86, 1),
    ("SDS", 0x5F, 0),
    ("SFVFS", 0x0B, 0),
    ("SFVTCA", 0x04, 1),
    ("SFVTL", 0x08, 1),
    ("SFVTPV", 0x0E, 0),
    ("SHC", 0x34, 1),
    ("SHP", 0x32, 1),
    ("SHPIX", 0x38, 0),
    ("SHZ", 0x36, 1),
    ("SLOOP", 0x17, 0),
    ("SMD", 0x1A, 0),
    ("SPVFS", 0x0A, 0),
    ("SPVTCA", 0x02, 1),
    ("SPVTL", 0x06, 1),
    ("SROUND", 0x76, 0),
    ("SRP0", 0x10, 0),
    ("SRP1", 0x11, 0),
    ("SRP2", 0x12, 0),
    ("SSW", 0x1F, 0),
    ("SSWCI", 0x1E, 0),
    ("SUB", 0x61, 0),
    ("SVTCA", 0x00, 1),
    ("SWAP", 0x23, 0),
    ("SZP0", 0x13, 0),
    ("SZP1", 0x14, 0),
    ("SZP2", 0x15, 0),
    ("SZPS", 0x16, 0),
    ("UTP", 0x29, 0),
    ("WCVTF", 0x70, 0),
    ("WCVTP", 0x44, 0),
    ("WS", 0x42, 0),
];

/// An assembly token: an instruction with its flag bits, or a number.
enum Token<'a> {
    Instruction { mnemonic: &'a str, bits: &'a str },
    Number(i32),
}

fn tokens(text: &str) -> Result<Vec<Token<'_>>, GftoolsError> {
    let error = |what: &str| GftoolsError::Misc(format!("TrueType assembly: {what}"));
    let mut out = vec![];
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(out);
        }
        if let Some(after) = rest.strip_prefix("/*") {
            let end = after.find("*/").ok_or_else(|| error("unclosed comment"))?;
            rest = &after[end + 2..];
        } else if rest.starts_with(|c: char| c.is_ascii_uppercase()) {
            let open = rest
                .find('[')
                .ok_or_else(|| error("instruction without [ ]"))?;
            let close = rest[open..].find(']').ok_or_else(|| error("unclosed ["))? + open;
            out.push(Token::Instruction {
                mnemonic: &rest[..open],
                bits: rest[open + 1..close].trim(),
            });
            rest = &rest[close + 1..];
        } else {
            let end = rest
                .find(|c: char| !(c == '-' || c.is_ascii_digit()))
                .unwrap_or(rest.len());
            let number = rest[..end]
                .parse()
                .map_err(|_| error(&format!("unexpected {:?}", &rest[..rest.len().min(12)])))?;
            out.push(Token::Number(number));
            rest = &rest[end..];
        }
    }
}

/// Assemble a TrueType program written in fontTools' assembly syntax (as
/// `ttProgram.Program.getAssembly` writes it). PUSHB, PUSHW, NPUSHB and NPUSHW
/// push exactly the numbers that follow them.
pub fn assemble(text: &str) -> Result<Vec<u8>, GftoolsError> {
    let error = |what: String| GftoolsError::Misc(format!("TrueType assembly: {what}"));
    let table: HashMap<&str, (u8, u32)> = INSTRUCTIONS
        .iter()
        .map(|(m, op, bits)| (*m, (*op, *bits)))
        .collect();
    let tokens = tokens(text)?;
    let mut code = vec![];
    let mut i = 0;
    while i < tokens.len() {
        let Token::Instruction { mnemonic, bits } = tokens[i] else {
            return Err(error("a number outside a push".to_string()));
        };
        i += 1;
        if matches!(mnemonic, "PUSHB" | "PUSHW" | "NPUSHB" | "NPUSHW") {
            let mut args = vec![];
            while let Some(Token::Number(n)) = tokens.get(i) {
                args.push(*n);
                i += 1;
            }
            let words = mnemonic.ends_with('W');
            match mnemonic {
                "PUSHB" | "PUSHW" => {
                    if args.is_empty() || args.len() > 8 {
                        return Err(error(format!("{mnemonic} of {} values", args.len())));
                    }
                    let base = if words { 0xB8 } else { 0xB0 };
                    code.push(base + args.len() as u8 - 1);
                }
                _ => {
                    if args.len() > 255 {
                        return Err(error(format!("{mnemonic} of {} values", args.len())));
                    }
                    code.push(if words { 0x41 } else { 0x40 });
                    code.push(args.len() as u8);
                }
            }
            for value in args {
                if words {
                    let value = i16::try_from(value)
                        .map_err(|_| error(format!("{value} does not fit a word")))?;
                    code.extend(value.to_be_bytes());
                } else {
                    let value = u8::try_from(value)
                        .map_err(|_| error(format!("{value} does not fit a byte")))?;
                    code.push(value);
                }
            }
            continue;
        }
        if let Some(op) = mnemonic.strip_prefix("INSTR") {
            code.push(op.parse().map_err(|_| error(format!("bad {mnemonic}")))?);
            continue;
        }
        let (op, n_bits) = table
            .get(mnemonic)
            .ok_or_else(|| error(format!("unknown instruction {mnemonic}")))?;
        if bits.len() as u32 != *n_bits {
            return Err(error(format!(
                "{mnemonic}[{bits}] needs {n_bits} flag bits"
            )));
        }
        let value = if bits.is_empty() {
            0
        } else {
            u8::from_str_radix(bits, 2)
                .map_err(|_| error(format!("bad bits in {mnemonic}[{bits}]")))?
        };
        code.push(op + value);
    }
    Ok(code)
}

fn matrix(transform: &Transform) -> [F2Dot14; 4] {
    [transform.xx, transform.yx, transform.xy, transform.yy]
}

/// A glyph's points as text: its advance, then each contour's points as
/// `x,y,on;` closed by `|`, or each component as `[` its own text, its F2Dot14
/// matrix and its offset `]`. Two glyphs with the same text give a TrueType
/// program the same points.
pub fn point_text(
    glyphs: &[Glyph],
    advances: &[u16],
    gid: usize,
    depth: usize,
) -> Result<String, GftoolsError> {
    if depth > 64 {
        return Err(GftoolsError::Misc(format!(
            "glyph {gid}: components nest too deeply"
        )));
    }
    let mut text = format!("w{}", advances.get(gid).copied().unwrap_or(0));
    match &glyphs[gid] {
        Glyph::Empty => {}
        Glyph::Simple(simple) => {
            for contour in &simple.contours {
                for point in contour.iter() {
                    text.push_str(&format!(
                        "{},{},{};",
                        point.x, point.y, point.on_curve as u8
                    ));
                }
                text.push('|');
            }
        }
        Glyph::Composite(composite) => {
            for component in composite.components() {
                let child = point_text(
                    glyphs,
                    advances,
                    component.glyph.to_u16() as usize,
                    depth + 1,
                )?;
                let m = matrix(&component.transform)
                    .iter()
                    .map(|v| v.to_bits().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let offset = match component.anchor {
                    Anchor::Offset { x, y } => format!("{x},{y}"),
                    Anchor::Point { base, component } => format!("p{base},{component}"),
                };
                text.push_str(&format!("[{child}({m};{offset})]"));
            }
        }
    }
    Ok(text)
}

/// A glyph's points as drawn, components placed by their transforms and offsets
/// (point-anchored components contribute nothing), as fontTools computes a
/// composite's box.
fn drawn_points(glyphs: &[Glyph], gid: usize, depth: usize) -> Vec<(f64, f64)> {
    if depth > 64 {
        return vec![];
    }
    match &glyphs[gid] {
        Glyph::Empty => vec![],
        Glyph::Simple(simple) => simple
            .contours
            .iter()
            .flat_map(|c| c.iter().map(|p| (f64::from(p.x), f64::from(p.y))))
            .collect(),
        Glyph::Composite(composite) => {
            let mut out = vec![];
            for component in composite.components() {
                let Anchor::Offset { x: dx, y: dy } = component.anchor else {
                    continue;
                };
                let t = &component.transform;
                let (xx, yx, xy, yy) = (
                    t.xx.to_f32() as f64,
                    t.yx.to_f32() as f64,
                    t.xy.to_f32() as f64,
                    t.yy.to_f32() as f64,
                );
                for (x, y) in drawn_points(glyphs, component.glyph.to_u16() as usize, depth + 1) {
                    out.push((
                        x * xx + y * xy + f64::from(dx),
                        x * yx + y * yy + f64::from(dy),
                    ));
                }
            }
            out
        }
    }
}

fn ot_round(v: f64) -> i16 {
    (v + 0.5).floor() as i16
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Rotate a contour to start at `want` (x, y, on-curve); true if it does, or if
/// its start is one the compiler dropped (`dropped`), put back later.
fn restore_start(points: &mut [CurvePoint], want: (i16, i16, u8), dropped: bool) -> bool {
    let at: Vec<usize> = points
        .iter()
        .enumerate()
        .filter(|(_, p)| (p.x, p.y) == (want.0, want.1) && p.on_curve == (want.2 != 0))
        .map(|(k, _)| k)
        .collect();
    let at = if at.is_empty() && dropped {
        // the start was dropped: begin after the two off-curves it lay between
        let n = points.len();
        (0..n)
            .filter(|&k| {
                let (p, q) = (points[(k + n - 1) % n], points[k]);
                !p.on_curve
                    && !q.on_curve
                    && p.x as i32 + q.x as i32 == 2 * want.0 as i32
                    && p.y as i32 + q.y as i32 == 2 * want.1 as i32
            })
            .collect()
    } else {
        at
    };
    match at.as_slice() {
        [k] => {
            points.rotate_left(*k);
            true
        }
        [0, ..] => true,
        _ => false,
    }
}

fn is_midpoint(p: &CurvePoint, prev: &CurvePoint, next: &CurvePoint) -> bool {
    p.on_curve
        && !prev.on_curve
        && !next.on_curve
        && prev.x as i32 + next.x as i32 == 2 * p.x as i32
        && prev.y as i32 + next.y as i32 == 2 * p.y as i32
}

fn restore_midpoints(contours: &mut [Contour], points: &[usize], release_ends: &[usize]) -> bool {
    if contours.len() != release_ends.len() {
        return false;
    }
    let mut lists: Vec<Vec<CurvePoint>> = contours
        .iter()
        .map(|c| c.iter().copied().collect())
        .collect();
    let mut sorted = points.to_vec();
    sorted.sort_unstable();
    for i in sorted {
        let Some(c) = release_ends.iter().position(|&e| i <= e) else {
            return false;
        };
        let start = if c == 0 { 0 } else { release_ends[c - 1] + 1 };
        let pos = i - start;
        let list = &mut lists[c];
        let n = list.len();
        if n == 0 {
            return false;
        }
        if pos < n && is_midpoint(&list[pos], &list[(pos + n - 1) % n], &list[(pos + 1) % n]) {
            continue; // kept by the compiler
        }
        let prev = list[(pos + n - 1) % n];
        let next = list[if pos < n { pos } else { 0 }];
        if prev.on_curve || next.on_curve {
            return false;
        }
        let (sx, sy) = (prev.x as i32 + next.x as i32, prev.y as i32 + next.y as i32);
        if sx % 2 != 0 || sy % 2 != 0 || pos > n {
            return false;
        }
        list.insert(pos, CurvePoint::on_curve((sx / 2) as i16, (sy / 2) as i16));
    }
    for (contour, list) in contours.iter_mut().zip(lists) {
        *contour = Contour::from(list);
    }
    true
}

fn set_component_flags(flags: &mut write_fonts::tables::glyf::ComponentFlags, bits: u16) {
    flags.round_xy_to_grid = bits & ROUND_XY_TO_GRID != 0;
    flags.use_my_metrics = bits & USE_MY_METRICS != 0;
    flags.overlap_compound = bits & OVERLAP_COMPOUND != 0;
    flags.scaled_component_offset = bits & SCALED_COMPONENT_OFFSET != 0;
    flags.unscaled_component_offset = bits & UNSCALED_COMPONENT_OFFSET != 0;
}

fn restore_transforms(glyph: &mut Glyph, matrices: &[[f64; 4]]) -> bool {
    let Glyph::Composite(composite) = glyph else {
        return false;
    };
    if composite.components().len() != matrices.len() {
        return false;
    }
    for (component, m) in composite.components().iter().zip(matrices) {
        let have = matrix(&component.transform);
        if have
            .iter()
            .zip(m)
            .any(|(a, b)| (a.to_f32() as f64 - b).abs() > 1.5 * F2DOT14_UNIT)
        {
            return false;
        }
    }
    for (component, m) in composite.components_mut().iter_mut().zip(matrices) {
        component.transform = Transform {
            xx: F2Dot14::from_f32(m[0] as f32),
            yx: F2Dot14::from_f32(m[1] as f32),
            xy: F2Dot14::from_f32(m[2] as f32),
            yy: F2Dot14::from_f32(m[3] as f32),
        };
    }
    true
}

fn raw_program(text: &Option<String>) -> Result<Option<Vec<u8>>, GftoolsError> {
    text.as_deref().map(assemble).transpose()
}

fn gasp_bytes(gasp: &BTreeMap<String, u16>) -> Result<Vec<u8>, GftoolsError> {
    let mut ranges: Vec<(u16, u16)> = gasp
        .iter()
        .map(|(k, v)| k.parse::<u16>().map(|k| (k, *v)))
        .collect::<Result<_, _>>()
        .map_err(|e| GftoolsError::Misc(format!("hinting file gasp: {e}")))?;
    ranges.sort();
    let mut out = vec![];
    out.extend(1u16.to_be_bytes());
    out.extend((ranges.len() as u16).to_be_bytes());
    for (ppem, behavior) in ranges {
        out.extend(ppem.to_be_bytes());
        out.extend(behavior.to_be_bytes());
    }
    Ok(out)
}

fn parse_key<T: std::str::FromStr>(key: &str, what: &str) -> Result<T, GftoolsError> {
    key.parse()
        .map_err(|_| GftoolsError::Misc(format!("hinting file {what}: bad key {key:?}")))
}

fn hdmx_bytes(
    hdmx: &BTreeMap<String, BTreeMap<String, u8>>,
    by_name: &HashMap<String, usize>,
    num_glyphs: usize,
) -> Result<Vec<u8>, GftoolsError> {
    let mut records: Vec<(u8, &BTreeMap<String, u8>)> = hdmx
        .iter()
        .map(|(k, w)| parse_key::<u8>(k, "hdmx").map(|p| (p, w)))
        .collect::<Result<_, _>>()?;
    records.sort_by_key(|(p, _)| *p);
    let size = (num_glyphs + 2 + 3) & !3;
    let mut out = vec![];
    out.extend(0u16.to_be_bytes());
    out.extend((records.len() as u16).to_be_bytes());
    out.extend((size as u32).to_be_bytes());
    for (ppem, widths) in records {
        let mut row = vec![0u8; num_glyphs];
        for (name, width) in widths {
            if let Some(&g) = by_name.get(name.as_str()) {
                row[g] = *width;
            }
        }
        out.push(ppem);
        out.push(row.iter().copied().max().unwrap_or(0));
        out.extend(&row);
        out.resize(out.len() + size - 2 - row.len(), 0);
    }
    Ok(out)
}

fn ltsh_bytes(
    ltsh: &BTreeMap<String, u8>,
    by_name: &HashMap<String, usize>,
    num_glyphs: usize,
) -> Vec<u8> {
    let mut y_pels = vec![1u8; num_glyphs];
    for (name, y) in ltsh {
        if let Some(&g) = by_name.get(name.as_str()) {
            y_pels[g] = *y;
        }
    }
    let mut out = vec![];
    out.extend(0u16.to_be_bytes());
    out.extend((num_glyphs as u16).to_be_bytes());
    out.extend(y_pels);
    out
}

fn vdmx_bytes(vdmx: &Vdmx) -> Result<Vec<u8>, GftoolsError> {
    let groups: Vec<Vec<(u16, (i16, i16))>> = vdmx
        .groups
        .iter()
        .map(|g| {
            let mut entries: Vec<(u16, (i16, i16))> = g
                .iter()
                .map(|(k, v)| parse_key::<u16>(k, "VDMX").map(|k| (k, *v)))
                .collect::<Result<_, _>>()?;
            entries.sort_by_key(|(k, _)| *k);
            Ok(entries)
        })
        .collect::<Result<_, GftoolsError>>()?;
    let n_ratios = vdmx.rat_ranges.len();
    let mut group_offsets = vec![];
    let mut pos = 6 + n_ratios * 4 + n_ratios * 2;
    for group in &groups {
        group_offsets.push(pos);
        pos += 4 + group.len() * 6;
    }
    let mut out = vec![];
    out.extend(vdmx.version.to_be_bytes());
    out.extend((groups.len() as u16).to_be_bytes());
    out.extend((n_ratios as u16).to_be_bytes());
    for r in &vdmx.rat_ranges {
        out.extend([r.b_char_set, r.x_ratio, r.y_start_ratio, r.y_end_ratio]);
    }
    for r in &vdmx.rat_ranges {
        let offset = group_offsets
            .get(r.group_index)
            .ok_or_else(|| GftoolsError::Misc("hinting file VDMX: bad groupIndex".into()))?;
        out.extend((*offset as u16).to_be_bytes());
    }
    for group in &groups {
        out.extend((group.len() as u16).to_be_bytes());
        out.push(group.first().map_or(0, |(k, _)| *k as u8));
        out.push(group.last().map_or(0, |(k, _)| *k as u8));
        for (height, (y_max, y_min)) in group {
            out.extend(height.to_be_bytes());
            out.extend(y_max.to_be_bytes());
            out.extend(y_min.to_be_bytes());
        }
    }
    Ok(out)
}

/// Give a compiled TrueType font the hinting of a hinting file.
pub fn apply_hinting(font_bytes: &[u8], file: &HintingFile) -> Result<Vec<u8>, GftoolsError> {
    let font = FontRef::new(font_bytes)?;
    let num_glyphs = font.maxp()?.num_glyphs() as usize;
    let names = GlyphNames::new(&font);
    let order: Vec<String> = (0..num_glyphs)
        .map(|gid| {
            names
                .get(GlyphId::new(gid as u32))
                .map(|n| n.to_string())
                .unwrap_or_default()
        })
        .collect();
    // the file's glyph names, matched by name, else by index (see `glyph_order`)
    let mut by_name: HashMap<String, usize> = order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();
    if file.glyph_order.len() == num_glyphs {
        for (i, n) in file.glyph_order.iter().enumerate() {
            by_name.entry(n.clone()).or_insert(i);
        }
    }
    let glyf = font.glyf()?;
    let loca = font.loca(None)?;
    let hmtx = font.hmtx()?;
    let advances: Vec<u16> = (0..num_glyphs)
        .map(|gid| hmtx.advance(GlyphId::new(gid as u32)).unwrap_or(0))
        .collect();
    let mut glyphs: Vec<Glyph> = (0..num_glyphs)
        .map(|gid| {
            Ok(match loca.get_glyf(GlyphId::new(gid as u32), &glyf)? {
                Some(glyph) => glyph.to_owned_table(),
                None => Glyph::Empty,
            })
        })
        .collect::<Result<_, GftoolsError>>()?;

    let mut bad: Vec<&str> = vec![];
    let mut lsbs: HashMap<usize, i16> = HashMap::new();
    for (name, entry) in &file.lsb_shifts {
        let gid = by_name.get(name.as_str()).copied();
        let ok = match gid.map(|g| &mut glyphs[g]) {
            Some(Glyph::Simple(simple)) => {
                let leftmost = simple
                    .contours
                    .iter()
                    .flat_map(|c| c.iter().map(|p| p.x))
                    .min()
                    .unwrap_or(entry.xmin);
                let shift = leftmost - entry.xmin;
                for contour in simple.contours.iter_mut() {
                    let list: Vec<CurvePoint> = contour
                        .iter()
                        .map(|p| CurvePoint::new(p.x - shift, p.y, p.on_curve))
                        .collect();
                    *contour = Contour::from(list);
                }
                let [x_min, y_min, x_max, y_max] = entry.bbox;
                simple.bbox = write_fonts::tables::glyf::Bbox {
                    x_min,
                    y_min,
                    x_max,
                    y_max,
                };
                true
            }
            _ => false,
        };
        if ok {
            lsbs.insert(gid.unwrap(), entry.lsb);
        } else {
            bad.push(name);
        }
    }
    for (name, offsets) in &file.offsets {
        let ok = match by_name.get(name.as_str()).map(|&g| &mut glyphs[g]) {
            Some(Glyph::Composite(composite)) if composite.components().len() == offsets.len() => {
                for (component, offset) in composite.components_mut().iter_mut().zip(offsets) {
                    if let (Some((x, y)), Anchor::Offset { .. }) = (offset, component.anchor) {
                        component.anchor = Anchor::Offset { x: *x, y: *y };
                    }
                }
                true
            }
            _ => false,
        };
        if !ok {
            bad.push(name);
        }
    }
    for (name, starts) in &file.contour_starts {
        let mid = file.midpoints.get(name);
        let ok =
            match by_name.get(name.as_str()).map(|&g| &mut glyphs[g]) {
                Some(Glyph::Simple(simple)) if simple.contours.len() == starts.len() => {
                    // contours whose release start is a midpoint the compiler dropped
                    let release_starts: Vec<usize> = mid.map_or(vec![], |m| {
                        std::iter::once(0)
                            .chain(m.ends.iter().map(|e| e + 1))
                            .take(m.ends.len())
                            .collect()
                    });
                    simple.contours.iter_mut().zip(starts).enumerate().all(
                        |(c, (contour, want))| {
                            let dropped = mid.is_some_and(|m| {
                                release_starts.get(c).is_some_and(|s| m.points.contains(s))
                            });
                            let mut list: Vec<CurvePoint> = contour.iter().copied().collect();
                            let ok = restore_start(&mut list, *want, dropped);
                            *contour = Contour::from(list);
                            ok
                        },
                    )
                }
                _ => false,
            };
        if !ok {
            bad.push(name);
        }
    }
    for (name, m) in &file.midpoints {
        let ok = match by_name.get(name.as_str()).map(|&g| &mut glyphs[g]) {
            Some(Glyph::Simple(simple)) => {
                restore_midpoints(&mut simple.contours, &m.points, &m.ends)
            }
            _ => false,
        };
        if !ok {
            bad.push(name);
        }
    }
    for (name, matrices) in &file.transforms {
        let ok = by_name
            .get(name.as_str())
            .is_some_and(|&g| restore_transforms(&mut glyphs[g], matrices));
        if !ok {
            bad.push(name);
        }
    }
    for (name, flags) in &file.components {
        let ok = match by_name.get(name.as_str()).map(|&g| &mut glyphs[g]) {
            Some(Glyph::Composite(composite)) if composite.components().len() == flags.len() => {
                for (component, bits) in composite.components_mut().iter_mut().zip(flags) {
                    set_component_flags(&mut component.flags, *bits);
                }
                true
            }
            _ => false,
        };
        if !ok {
            bad.push(name);
        }
    }
    for (name, program) in &file.glyphs {
        let ok = match by_name.get(name.as_str()) {
            Some(&g) => digest(&point_text(&glyphs, &advances, g, 0)?) == program.id,
            None => false,
        };
        if !ok {
            bad.push(name);
        }
    }
    if !bad.is_empty() {
        bad.sort_unstable();
        bad.dedup();
        return Err(GftoolsError::Misc(format!(
            "{} glyph(s) do not have the points or components the hinting file was written for: {}",
            bad.len(),
            bad.iter().take(20).copied().collect::<Vec<_>>().join(" ")
        )));
    }

    // composites' boxes from their components' points, as a compiler computes
    // them (restorations may have moved those points); recorded boxes stay
    let recorded: std::collections::HashSet<usize> = file
        .boxes
        .keys()
        .filter_map(|n| by_name.get(n.as_str()).copied())
        .collect();
    for gid in 0..num_glyphs {
        if recorded.contains(&gid) || !matches!(glyphs[gid], Glyph::Composite(_)) {
            continue;
        }
        let points = drawn_points(&glyphs, gid, 0);
        if let (Glyph::Composite(composite), false) = (&mut glyphs[gid], points.is_empty()) {
            let fold = |f: fn(f64, f64) -> f64, init: f64, sel: fn(&(f64, f64)) -> f64| {
                points.iter().map(sel).fold(init, f)
            };
            composite.bbox = write_fonts::tables::glyf::Bbox {
                x_min: ot_round(fold(f64::min, f64::INFINITY, |p| p.0)),
                y_min: ot_round(fold(f64::min, f64::INFINITY, |p| p.1)),
                x_max: ot_round(fold(f64::max, f64::NEG_INFINITY, |p| p.0)),
                y_max: ot_round(fold(f64::max, f64::NEG_INFINITY, |p| p.1)),
            };
        }
    }
    for (name, entry) in &file.boxes {
        if let Some(&gid) = by_name.get(name.as_str())
            && let Glyph::Composite(composite) = &mut glyphs[gid]
        {
            let [x_min, y_min, x_max, y_max] = entry.bbox;
            composite.bbox = write_fonts::tables::glyf::Bbox {
                x_min,
                y_min,
                x_max,
                y_max,
            };
            lsbs.insert(gid, entry.lsb);
        }
    }

    let programs: HashMap<usize, &GlyphProgram> = file
        .glyphs
        .iter()
        .filter_map(|(n, p)| by_name.get(n.as_str()).map(|&g| (g, p)))
        .collect();
    let mut builder = GlyfLocaBuilder::new();
    for (gid, glyph) in glyphs.iter_mut().enumerate() {
        let code = match programs.get(&gid) {
            Some(program) => assemble(&program.assembly)?,
            None => vec![],
        };
        match glyph {
            Glyph::Simple(simple) => simple.instructions = code,
            Glyph::Composite(composite) => composite.set_instructions(&code),
            Glyph::Empty => {}
        }
        builder.add_glyph(glyph)?;
    }
    let (new_glyf, new_loca, loca_format) = builder.build();

    let mut head: Head = font.head()?.to_owned_table();
    head.index_to_loc_format = loca_format as i16;
    head.flags = if file.head_flag_3 {
        head.flags | write_fonts::tables::head::Flags::FORCE_INTEGER_PPEM
    } else {
        head.flags & !write_fonts::tables::head::Flags::FORCE_INTEGER_PPEM
    };
    if let Some(bits) = file.head_flags_2_4 {
        let kept = head.flags.bits() & !0x14;
        head.flags = write_fonts::tables::head::Flags::from_bits_truncate(kept | (bits & 0x14));
    }
    let mut maxp: Maxp = font.maxp()?.to_owned_table();
    for (key, value) in &file.maxp {
        let field = match key.as_str() {
            "maxZones" => &mut maxp.max_zones,
            "maxTwilightPoints" => &mut maxp.max_twilight_points,
            "maxStorage" => &mut maxp.max_storage,
            "maxFunctionDefs" => &mut maxp.max_function_defs,
            "maxInstructionDefs" => &mut maxp.max_instruction_defs,
            "maxStackElements" => &mut maxp.max_stack_elements,
            "maxSizeOfInstructions" => &mut maxp.max_size_of_instructions,
            _ => {
                return Err(GftoolsError::Misc(format!(
                    "hinting file maxp: unknown {key}"
                )));
            }
        };
        *field = Some(*value);
    }

    let mut hmtx_owned: write_fonts::tables::hmtx::Hmtx = font.hmtx()?.to_owned_table();
    for (gid, lsb) in &lsbs {
        let n_long = hmtx_owned.h_metrics.len();
        if *gid < n_long {
            hmtx_owned.h_metrics[*gid].side_bearing = *lsb;
        } else if let Some(v) = hmtx_owned.left_side_bearings.get_mut(gid - n_long) {
            *v = *lsb;
        }
    }

    let mut new_font = FontBuilder::new();
    new_font.add_table(&new_glyf)?;
    new_font.add_table(&new_loca)?;
    new_font.add_table(&head)?;
    new_font.add_table(&maxp)?;
    new_font.add_table(&hmtx_owned)?;
    let mut raw: Vec<(Tag, Vec<u8>)> = vec![];
    if let Some(code) = raw_program(&file.fpgm)? {
        raw.push((Tag::new(b"fpgm"), code));
    }
    if let Some(code) = raw_program(&file.prep)? {
        raw.push((Tag::new(b"prep"), code));
    }
    if let Some(cvt) = &file.cvt {
        raw.push((
            Tag::new(b"cvt "),
            cvt.iter().flat_map(|v| v.to_be_bytes()).collect(),
        ));
    }
    if let Some(gasp) = &file.gasp {
        raw.push((Tag::new(b"gasp"), gasp_bytes(gasp)?));
    }
    if let Some(hdmx) = &file.hdmx {
        raw.push((Tag::new(b"hdmx"), hdmx_bytes(hdmx, &by_name, num_glyphs)?));
    }
    if let Some(ltsh) = &file.ltsh {
        raw.push((Tag::new(b"LTSH"), ltsh_bytes(ltsh, &by_name, num_glyphs)));
    }
    if let Some(vdmx) = &file.vdmx {
        raw.push((Tag::new(b"VDMX"), vdmx_bytes(vdmx)?));
    }
    // tables the file states, or states the absence of, are not copied
    let replaced = [
        "glyf", "loca", "head", "maxp", "hmtx", "fpgm", "prep", "cvt ", "gasp", "hdmx", "LTSH",
        "VDMX",
    ];
    for (tag, data) in &raw {
        new_font.add_raw(*tag, data.clone());
    }
    for record in font.table_directory.table_records() {
        let tag = record.tag();
        if replaced
            .iter()
            .any(|t| Tag::new_checked(t.as_bytes()).ok() == Some(tag))
        {
            // gasp stays when the file does not state it
            if tag == Tag::new(b"gasp")
                && file.gasp.is_none()
                && let Some(data) = font.table_data(tag)
            {
                new_font.add_raw(tag, data.as_bytes().to_vec());
            }
            continue;
        }
        if let Some(data) = font.table_data(tag) {
            new_font.add_raw(tag, data.as_bytes().to_vec());
        }
    }
    Ok(new_font.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_pushes_and_flag_bits() {
        let text = "SVTCA[0]\t/* SetFPVectorToAxis */\nPUSHW[ ]\t/* 2 values pushed */\n8\n-3\n\
                    PUSHB[ ]\t/* 1 value pushed */\n17\nCALL[ ]\nMIRP[10110]\nIUP[1]";
        assert_eq!(
            assemble(text).unwrap(),
            vec![
                0x00,
                0xB9,
                0x00,
                0x08,
                0xFF,
                0xFD,
                0xB0,
                17,
                0x2B,
                0xE0 + 0b10110,
                0x31
            ]
        );
    }

    #[test]
    fn rejects_wrong_flag_bits() {
        assert!(assemble("MIRP[1]").is_err());
        assert!(assemble("NOPE[ ]").is_err());
    }

    #[test]
    fn restores_a_dropped_midpoint() {
        // a square of off-curves with an on-curve at the top middle, then dropped
        let full = vec![
            CurvePoint::off_curve(0, 0),
            CurvePoint::off_curve(0, 100),
            CurvePoint::on_curve(50, 100),
            CurvePoint::off_curve(100, 100),
            CurvePoint::off_curve(100, 0),
        ];
        let mut dropped: Vec<CurvePoint> = full.clone();
        dropped.remove(2);
        let mut contours = vec![Contour::from(dropped)];
        assert!(restore_midpoints(&mut contours, &[2], &[4]));
        assert_eq!(contours[0].iter().copied().collect::<Vec<_>>(), full);
        // nothing to do when the point is there
        assert!(restore_midpoints(&mut contours, &[2], &[4]));
        assert_eq!(contours[0].len(), 5);
    }
}
