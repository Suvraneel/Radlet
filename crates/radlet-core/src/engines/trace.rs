//! Tracing pictures into SVG drawings: logos, icons, signatures and line
//! art redrawn as filled shapes that stay sharp at any size.
//!
//! The shapes come from visioncortex (MIT OR Apache-2.0): its colour
//! clustering splits the picture into areas of one colour, stacked largest
//! first, and each area's outline becomes a smoothed path. The steps and
//! settings follow those of visioncortex's own VTracer (also MIT OR
//! Apache-2.0); this module writes the SVG itself.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{imageops::FilterType, DynamicImage, GenericImageView};
use visioncortex::color_clusters::{KeyingAction, Runner, RunnerConfig, HIERARCHICAL_MAX};
use visioncortex::{Color, ColorImage, CompoundPath, PathSimplifyMode, PointF64};

use crate::error::{Error, Result};

/// The longest side traced. Tracing works on shapes, not pixels, so a
/// larger picture only adds specks and time; the drawing still reports the
/// picture's own size.
pub const MAX_SIDE: u32 = 2048;

/// Largest drawing written (64 MB). A photo traced in detail can run past
/// it; Simple or Black and white keep it small.
const MAX_SVG_BYTES: usize = 64 * 1024 * 1024;

pub use crate::ops::TraceDetail;

/// The settings behind each level of detail.
struct Settings {
    /// Areas smaller than this many pixels are dropped as specks.
    min_area: usize,
    /// Colour bits ignored when deciding two pixels are the same colour.
    precision_loss: i32,
    /// How different stacked layers must be to stay apart.
    layer_difference: i32,
    /// Corners sharper than this (degrees) stay corners.
    corner_degrees: f64,
    /// Shortest curve segment, in pixels.
    segment_length: f64,
    /// Decimal places in path coordinates.
    precision: u32,
}

impl TraceDetail {
    fn settings(self) -> Settings {
        match self {
            TraceDetail::Low => Settings {
                min_area: 36,
                precision_loss: 3,
                layer_difference: 24,
                corner_degrees: 75.0,
                segment_length: 5.0,
                precision: 1,
            },
            TraceDetail::Medium => Settings {
                min_area: 16,
                precision_loss: 2,
                layer_difference: 16,
                corner_degrees: 60.0,
                segment_length: 4.0,
                precision: 2,
            },
            TraceDetail::High => Settings {
                min_area: 4,
                precision_loss: 1,
                layer_difference: 8,
                corner_degrees: 45.0,
                segment_length: 3.5,
                precision: 2,
            },
        }
    }
}

/// How smoothly outlines are fitted, and where curves join.
const MAX_ITERATIONS: usize = 10;
const SPLICE_DEGREES: f64 = 45.0;

/// Traces `img` into an SVG document. With `color`, every area keeps its
/// colour; without, dark areas become black shapes and the rest is left
/// empty (transparent). Transparent parts of the picture stay transparent.
pub fn trace(
    img: &DynamicImage,
    color: bool,
    detail: TraceDetail,
    cancel: &AtomicBool,
) -> Result<String> {
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Invalid("the picture is empty".into()));
    }
    let longest = width.max(height);
    let work = if longest > MAX_SIDE {
        let scale = MAX_SIDE as f64 / longest as f64;
        let w = ((width as f64 * scale).round() as u32).max(1);
        let h = ((height as f64 * scale).round() as u32).max(1);
        img.resize_exact(w, h, FilterType::Lanczos3)
    } else {
        img.clone()
    };
    let rgba = work.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let image = ColorImage {
        pixels: rgba.into_raw(),
        width: w,
        height: h,
    };
    let settings = detail.settings();
    let shapes = if color {
        color_shapes(image, &settings, cancel)?
    } else {
        black_shapes(&image, &settings, cancel)?
    };
    let svg = write_svg(&shapes, (width, height), (w, h), settings.precision);
    if svg.len() > MAX_SVG_BYTES {
        return Err(Error::TooLarge(
            "this picture has too much detail to trace well; try Simple or Black and white".into(),
        ));
    }
    Ok(svg)
}

/// One filled shape: its outline and colour.
struct Shape {
    path: CompoundPath,
    fill: Color,
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn color_shapes(
    mut image: ColorImage,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Vec<Shape>> {
    let (w, h) = (image.width, image.height);
    // Transparent pixels are painted one colour the picture doesn't use,
    // and areas of that colour are left out of the drawing.
    let key = if has_transparency(&image) {
        let key = unused_color(&image)
            .ok_or_else(|| Error::Invalid("this picture uses every colour there is".into()))?;
        for y in 0..h {
            for x in 0..w {
                if image.get_pixel(x, y).a < 128 {
                    image.set_pixel(x, y, &key);
                }
            }
        }
        key
    } else {
        // All zeroes means no keying to visioncortex.
        Color::default()
    };
    // Every other pixel is treated as opaque.
    for px in image.pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    let runner = Runner::new(
        RunnerConfig {
            diagonal: settings.layer_difference == 0,
            hierarchical: HIERARCHICAL_MAX,
            batch_size: 25_600,
            good_min_area: settings.min_area,
            good_max_area: w * h,
            is_same_color_a: settings.precision_loss,
            is_same_color_b: 1,
            deepen_diff: settings.layer_difference,
            hollow_neighbours: 1,
            key_color: key,
            keying_action: KeyingAction::Discard,
        },
        image,
    );
    let mut builder = runner.start();
    loop {
        cancelled(cancel)?;
        if builder.tick() {
            break;
        }
    }
    let clusters = builder.result();
    let view = clusters.view();
    let mut shapes = Vec::new();
    // Largest areas first, so smaller ones are drawn on top.
    for &index in view.clusters_output.iter().rev() {
        cancelled(cancel)?;
        let cluster = view.get_cluster(index);
        let path = cluster.to_compound_path(
            &view,
            false,
            PathSimplifyMode::Spline,
            settings.corner_degrees.to_radians(),
            settings.segment_length,
            MAX_ITERATIONS,
            SPLICE_DEGREES.to_radians(),
        );
        shapes.push(Shape {
            path,
            fill: cluster.residue_color(),
        });
    }
    Ok(shapes)
}

fn black_shapes(
    image: &ColorImage,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Vec<Shape>> {
    // Dark and not transparent: luma below the middle.
    let binary = image.to_binary_image(|c| {
        let luma = (299 * c.r as u32 + 587 * c.g as u32 + 114 * c.b as u32) / 1000;
        c.a >= 128 && luma < 128
    });
    let clusters = binary.to_clusters(false);
    let mut shapes = Vec::new();
    for i in 0..clusters.len() {
        cancelled(cancel)?;
        let cluster = clusters.get_cluster(i);
        if cluster.size() < settings.min_area {
            continue;
        }
        let path = cluster.to_compound_path(
            PathSimplifyMode::Spline,
            settings.corner_degrees.to_radians(),
            settings.segment_length,
            MAX_ITERATIONS,
            SPLICE_DEGREES.to_radians(),
        );
        shapes.push(Shape {
            path,
            fill: Color::new(0, 0, 0),
        });
    }
    Ok(shapes)
}

/// True when any of the picture is see-through. Those pixels are left out
/// of the drawing rather than traced in whatever colour they hide.
fn has_transparency(image: &ColorImage) -> bool {
    image.pixels.as_chunks::<4>().0.iter().any(|p| p[3] < 128)
}

/// A colour no opaque pixel uses, to stand for transparency.
fn unused_color(image: &ColorImage) -> Option<Color> {
    let mut used = vec![false; 1 << 15];
    let bucket = |r: u8, g: u8, b: u8| {
        ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3)
    };
    for p in image.pixels.as_chunks::<4>().0 {
        if p[3] >= 128 {
            used[bucket(p[0], p[1], p[2])] = true;
        }
    }
    // Prefer loud colours few pictures use; then any free bucket. A whole
    // bucket being free keeps clustering's colour tolerance off it too.
    let loud = [
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 0),
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
    ];
    loud.iter()
        .find(|&&(r, g, b)| !used[bucket(r, g, b)])
        .map(|&(r, g, b)| Color::new(r, g, b))
        .or_else(|| {
            used.iter().position(|u| !u).map(|i| {
                let r = ((i >> 10) & 31) as u8;
                let g = ((i >> 5) & 31) as u8;
                let b = (i & 31) as u8;
                Color::new(r << 3 | 4, g << 3 | 4, b << 3 | 4)
            })
        })
}

/// The drawing, sized as the original picture (`size`) with coordinates in
/// the traced picture's pixels (`traced`).
fn write_svg(shapes: &[Shape], size: (u32, u32), traced: (usize, usize), precision: u32) -> String {
    let mut svg = String::new();
    svg.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">",
        size.0, size.1, traced.0, traced.1
    );
    for shape in shapes {
        let (d, offset) = shape
            .path
            .to_svg_string(true, PointF64::default(), Some(precision));
        if d.is_empty() {
            continue;
        }
        let _ = writeln!(
            svg,
            "<path d=\"{d}\" fill=\"{}\" transform=\"translate({},{})\"/>",
            shape.fill.to_hex_string(),
            offset.x,
            offset.y
        );
    }
    svg.push_str("</svg>\n");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A white square with a red disc and a blue bar.
    fn logo(size: u32) -> DynamicImage {
        let mut img = RgbaImage::from_pixel(size, size, Rgba([255, 255, 255, 255]));
        let c = size as f64 / 2.0;
        for y in 0..size {
            for x in 0..size {
                let (dx, dy) = (x as f64 - c, y as f64 - c);
                if (dx * dx + dy * dy).sqrt() < size as f64 / 4.0 {
                    img.put_pixel(x, y, Rgba([220, 30, 30, 255]));
                }
                if y > size * 3 / 4 && y < size * 7 / 8 {
                    img.put_pixel(x, y, Rgba([20, 40, 200, 255]));
                }
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    fn fills(svg: &str) -> Vec<String> {
        svg.match_indices("fill=\"#")
            .map(|(i, _)| svg[i + 6..i + 13].to_string())
            .collect()
    }

    #[test]
    fn colour_traces_keep_their_colours() {
        let svg = trace(&logo(160), true, TraceDetail::Medium, &never()).unwrap();
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("width=\"160\" height=\"160\" viewBox=\"0 0 160 160\""));
        let fills = fills(&svg);
        assert!(fills.len() >= 3, "{fills:?}");
        // The disc and the bar come out near their own colours.
        let close = |want: (i32, i32, i32)| {
            fills.iter().any(|f| {
                let v = |i: usize| i32::from_str_radix(&f[i..i + 2], 16).unwrap();
                (v(1) - want.0).abs() + (v(3) - want.1).abs() + (v(5) - want.2).abs() < 60
            })
        };
        assert!(close((220, 30, 30)), "{fills:?}");
        assert!(close((20, 40, 200)), "{fills:?}");
        // Paths are curves, not pixel staircases.
        assert!(svg.contains(" C"), "spline segments");
    }

    #[test]
    fn black_and_white_draws_only_the_dark_parts() {
        let svg = trace(&logo(160), false, TraceDetail::Medium, &never()).unwrap();
        let fills = fills(&svg);
        assert!(!fills.is_empty());
        assert!(fills.iter().all(|f| f == "#000000"), "{fills:?}");
        // The red disc and the blue bar are dark; the white isn't drawn.
        assert_eq!(fills.len(), 2, "{svg}");
    }

    #[test]
    fn transparency_stays_out_of_the_drawing() {
        let mut img = RgbaImage::from_pixel(120, 120, Rgba([0, 0, 0, 0]));
        for y in 30..90 {
            for x in 30..90 {
                img.put_pixel(x, y, Rgba([10, 160, 60, 255]));
            }
        }
        let svg = trace(
            &DynamicImage::ImageRgba8(img),
            true,
            TraceDetail::Medium,
            &never(),
        )
        .unwrap();
        let fills = fills(&svg);
        assert_eq!(fills.len(), 1, "only the green square: {fills:?}");
    }

    #[test]
    fn big_pictures_are_traced_smaller_but_keep_their_size() {
        let img = logo(64).resize_exact(4096, 2048, FilterType::Nearest);
        let svg = trace(&img, true, TraceDetail::Low, &never()).unwrap();
        assert!(
            svg.contains("width=\"4096\" height=\"2048\" viewBox=\"0 0 2048 1024\""),
            "{}",
            &svg[..200]
        );
    }

    #[test]
    fn less_detail_means_fewer_shapes() {
        // Noisy gradient: lots of near-colours.
        let mut img = RgbaImage::new(128, 128);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let n = ((x * 7 + y * 13) % 11) as u8;
            *p = Rgba([
                (x * 2) as u8 ^ n,
                (y * 2) as u8,
                128u8.wrapping_add(n * 3),
                255,
            ]);
        }
        let img = DynamicImage::ImageRgba8(img);
        let low = trace(&img, true, TraceDetail::Low, &never()).unwrap();
        let high = trace(&img, true, TraceDetail::High, &never()).unwrap();
        assert!(fills(&low).len() < fills(&high).len());
    }

    #[test]
    fn cancelling_stops_the_trace() {
        let stop = AtomicBool::new(true);
        assert!(matches!(
            trace(&logo(64), true, TraceDetail::Medium, &stop),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn unused_colours_avoid_the_picture() {
        let img = ColorImage {
            pixels: [255u8, 0, 255, 255].repeat(4),
            width: 2,
            height: 2,
        };
        let key = unused_color(&img).unwrap();
        assert_ne!((key.r, key.g, key.b), (255, 0, 255));
    }
}
