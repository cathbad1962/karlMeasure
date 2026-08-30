//! Draws the application's mark and puts it where both the window and the
//! executable can carry it.
//!
//! The mark is what the application does: a traced area, its fill carrying the
//! outline blue, an anchor at each corner in the colour a selected one takes.
//! It is drawn rather than decoded, so there is no image in the repository and
//! no library to read one.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The traced outline, in a square of side 1. Deliberately not a rectangle: a
/// measured area is whatever shape the drawing gives it, and the slant is what
/// says so at a glance.
const OUTLINE: [(f64, f64); 4] = [(0.14, 0.20), (0.86, 0.28), (0.80, 0.82), (0.18, 0.74)];

/// The area's fill, its edge, and the anchors: the interface's own colours.
const FILL: [u8; 4] = [0, 150, 190, 215];
const EDGE: [u8; 4] = [0, 120, 155, 255];
const ANCHOR: [u8; 4] = [255, 210, 80, 255];

/// Widths and radii, as a fraction of the side, so the mark scales whole.
const EDGE_WIDTH: f64 = 0.055;
const ANCHOR_RADIUS: f64 = 0.075;

/// Every size the executable's icon carries. 256 is what a large view shows;
/// 16 is a title bar.
const SIZES: [u32; 5] = [16, 24, 32, 48, 256];

/// The side of the icon handed to the window layer, which scales it itself.
const WINDOW_ICON: u32 = 256;

/// Samples per pixel along each axis. Sixteen samples is enough to keep a
/// slanted edge smooth at the sizes that matter.
const SAMPLES: u32 = 4;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));

    // The window's icon, embedded by the binary at compile time.
    let window = draw(WINDOW_ICON);
    write(&out_dir.join("icon.rgba"), &window);

    // The executable's icon, which Explorer and the taskbar read from the file
    // itself rather than from anything the program does.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico = out_dir.join("icon.ico");
        write(&ico, &icon_file());

        let script = out_dir.join("icon.rc");
        write(
            &script,
            format!(
                "1 ICON \"{}\"\n",
                ico.display().to_string().replace('\\', "\\\\")
            )
            .as_bytes(),
        );

        embed_resource::compile(&script, embed_resource::NONE)
            .manifest_required()
            .expect("the icon is compiled into the executable");
    }

    println!("cargo:rerun-if-changed=build.rs");
}

fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
}

/// The mark at `size` square, as tightly packed RGBA.
fn draw(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let side = f64::from(size);

    for y in 0..size {
        for x in 0..size {
            // Coverage is counted over a grid of samples within the pixel,
            // which is what keeps the slanted edges from stepping.
            let (mut fill, mut edge, mut anchor) = (0.0, 0.0, 0.0);

            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let at = (
                        (f64::from(x) + (f64::from(sx) + 0.5) / f64::from(SAMPLES)) / side,
                        (f64::from(y) + (f64::from(sy) + 0.5) / f64::from(SAMPLES)) / side,
                    );

                    if near_a_corner(at) {
                        anchor += 1.0;
                    } else if near_the_edge(at) {
                        edge += 1.0;
                    } else if inside(at) {
                        fill += 1.0;
                    }
                }
            }

            let total = f64::from(SAMPLES * SAMPLES);
            let at = ((y * size + x) * 4) as usize;

            // Painted in the order they sit on top of one another, so an
            // anchor covers the edge and the edge covers the fill.
            for (colour, coverage) in [(FILL, fill), (EDGE, edge), (ANCHOR, anchor)] {
                over(&mut pixels[at..at + 4], colour, coverage / total);
            }
        }
    }

    pixels
}

/// Lays `colour` over what is already there, at `coverage` of its own alpha.
fn over(pixel: &mut [u8], colour: [u8; 4], coverage: f64) {
    if coverage <= 0.0 {
        return;
    }

    let alpha = coverage * f64::from(colour[3]) / 255.0;

    for channel in 0..3 {
        let under = f64::from(pixel[channel]);
        let over = f64::from(colour[channel]);
        pixel[channel] = (over * alpha + under * (1.0 - alpha)).round() as u8;
    }

    let under = f64::from(pixel[3]) / 255.0;
    pixel[3] = ((alpha + under * (1.0 - alpha)) * 255.0).round() as u8;
}

/// Whether a point falls within the outline, by the winding of the edges
/// around it. The outline is convex, so the sign of every cross product is the
/// same for a point inside it.
fn inside(at: (f64, f64)) -> bool {
    OUTLINE.iter().enumerate().all(|(index, from)| {
        let to = OUTLINE[(index + 1) % OUTLINE.len()];
        let edge = (to.0 - from.0, to.1 - from.1);
        let corner = (at.0 - from.0, at.1 - from.1);

        edge.0 * corner.1 - edge.1 * corner.0 >= 0.0
    })
}

fn near_the_edge(at: (f64, f64)) -> bool {
    OUTLINE.iter().enumerate().any(|(index, from)| {
        let to = OUTLINE[(index + 1) % OUTLINE.len()];

        distance_to_segment(at, *from, to) <= EDGE_WIDTH / 2.0
    })
}

fn near_a_corner(at: (f64, f64)) -> bool {
    OUTLINE
        .iter()
        .any(|corner| hypot(at, *corner) <= ANCHOR_RADIUS)
}

fn hypot(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn distance_to_segment(at: (f64, f64), from: (f64, f64), to: (f64, f64)) -> f64 {
    let along = (to.0 - from.0, to.1 - from.1);
    let length = along.0 * along.0 + along.1 * along.1;

    if length <= f64::EPSILON {
        return hypot(at, from);
    }

    let t = (((at.0 - from.0) * along.0 + (at.1 - from.1) * along.1) / length).clamp(0.0, 1.0);

    hypot(at, (from.0 + along.0 * t, from.1 + along.1 * t))
}

/// The mark at every size, in the icon format Windows reads: a directory of
/// entries, each a bitmap of its own.
fn icon_file() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&size| (size, bitmap(size))).collect();

    let mut file = Vec::new();
    file.extend([0, 0]); // Reserved.
    file.extend([1, 0]); // An icon rather than a cursor.
    file.extend(
        u16::try_from(images.len())
            .expect("a handful of sizes")
            .to_le_bytes(),
    );

    // The images follow the directory, one after another.
    let mut offset = 6 + 16 * images.len() as u32;

    for (size, bitmap) in &images {
        // 256 is written as zero: the field is one byte wide.
        let side = u8::try_from(*size).unwrap_or(0);

        file.push(side);
        file.push(side);
        file.extend([0, 0]); // No palette, no flags.
        file.extend(1u16.to_le_bytes()); // One colour plane.
        file.extend(32u16.to_le_bytes()); // Bits per pixel.
        file.extend(
            u32::try_from(bitmap.len())
                .expect("a small image")
                .to_le_bytes(),
        );
        file.extend(offset.to_le_bytes());

        offset += bitmap.len() as u32;
    }

    for (_, bitmap) in &images {
        file.extend(bitmap);
    }

    file
}

/// One entry of the icon: a bitmap header, the pixels bottom-up in BGRA, and
/// the mask that older readers expect. The alpha channel is what actually
/// carries transparency, so the mask is left clear.
fn bitmap(size: u32) -> Vec<u8> {
    let pixels = draw(size);
    let mut entry = Vec::new();

    entry.extend(40u32.to_le_bytes()); // Header size.
    entry.extend(i32::try_from(size).expect("a small image").to_le_bytes());
    // Twice the height: the format counts the colour and the mask together.
    entry.extend((i32::try_from(size).expect("a small image") * 2).to_le_bytes());
    entry.extend(1u16.to_le_bytes()); // One colour plane.
    entry.extend(32u16.to_le_bytes()); // Bits per pixel.
    entry.extend([0u8; 24]); // No compression, no palette, sizes left at zero.

    for y in (0..size).rev() {
        for x in 0..size {
            let at = ((y * size + x) * 4) as usize;
            let pixel = &pixels[at..at + 4];

            entry.extend([pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }

    // The mask: one bit per pixel, each row padded out to four bytes.
    let row = (size.div_ceil(32) * 4) as usize;
    entry.extend(vec![0u8; row * size as usize]);

    entry
}
