//! Geometry invariants, quantization and clamping.

use super::support::{assert_outcome, Expect, SIZE};
use dcmview_annotation::{quantize, Geometry, Point, Snap, ViolationCode};
use serde_json::{json, Value};

fn geometry(value: Value) -> Geometry {
    serde_json::from_value(value).expect("geometry reads")
}

#[test]
fn coordinates_quantize_to_a_thousandth_of_a_pixel() {
    let cases: &[(f64, f64)] = &[
        (0.0, 0.0),
        (12.0, 12.0),
        (12.3456, 12.346),
        (12.3454, 12.345),
        (0.0005, 0.001),
        (-0.0005, -0.001),
        (0.1 + 0.2, 0.3),
        (239.9996, 240.0),
        (-7.25, -7.25),
    ];
    for (value, expected) in cases {
        assert_eq!(quantize(*value), *expected, "quantize({value})");
    }
    // Anything that rounds to zero is positive zero.
    for value in [-0.0, -0.0004, 0.0004] {
        let quantized = quantize(value);
        assert_eq!(quantized, 0.0);
        assert!(quantized.is_sign_positive(), "quantize({value})");
    }
    assert!(quantize(f64::NAN).is_nan());
    assert_eq!(quantize(f64::INFINITY), f64::INFINITY);
    // A finite value too large to scale by 1000 stays as it is, so it can
    // still be written and is refused as outside the image, not as a number
    // that is not finite.
    for value in [1e306, -1e306, f64::MAX, f64::MIN] {
        assert_eq!(quantize(value), value, "quantize({value:e})");
        let point = Geometry::Point { x: value, y: 1.0 };
        let text = serde_json::to_string(&point).expect("a finite coordinate serializes");
        assert_eq!(
            serde_json::from_str::<Geometry>(&text).expect("and reads back"),
            point
        );
        let invalid = point.validate(SIZE).expect_err("outside the image");
        assert!(
            invalid.has(ViolationCode::OutOfBounds) && !invalid.has(ViolationCode::NonFinite),
            "{value:e}: {:?}",
            invalid.violations
        );
    }

    let polygon = Geometry::Polygon {
        points: vec![
            Point { x: 1.00049, y: 2.0 },
            Point {
                x: 7.14159,
                y: 4.71828,
            },
            Point { x: 0.0, y: 9.9996 },
        ],
    };
    assert_eq!(
        polygon.quantized(),
        Geometry::Polygon {
            points: vec![
                Point { x: 1.0, y: 2.0 },
                Point { x: 7.142, y: 4.718 },
                Point { x: 0.0, y: 10.0 },
            ],
        }
    );
}

/// The strict check, against an image of 160 rows by 240 columns with three
/// frames. Both ends of the range are valid: a shape may end on the far edge.
#[test]
fn a_geometry_is_checked_against_its_image() {
    use Expect::{Code, Valid};
    use ViolationCode::*;

    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| json!({ "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 });
    let ellipse = |cx: f64, cy: f64, rx: f64, ry: f64, angle: f64| json!({ "type": "ellipse", "cx": cx, "cy": cy, "rx": rx, "ry": ry, "angle": angle });
    let points = |kind: &str, list: &[(f64, f64)]| json!({ "type": kind, "points": list.iter().map(|(x, y)| json!({ "x": x, "y": y })).collect::<Vec<_>>() });
    let mask = |tile: u32, depth: u32, frames: Value| json!({ "type": "mask", "encoding": "tiles-v1", "tile": tile, "depth": depth, "frames": frames });

    let cases: Vec<(&str, Value, Expect)> = vec![
        (
            "rect over the whole image",
            rect(0.0, 0.0, 240.0, 160.0),
            Valid,
        ),
        ("rect inside", rect(30.5, 20.25, 90.125, 60.0), Valid),
        (
            "rect within half a quantum of the edge",
            rect(0.0, 0.0, 240.0004, 160.0),
            Valid,
        ),
        (
            "rect one quantum past the last column",
            rect(0.0, 0.0, 240.001, 160.0),
            Code(OutOfBounds),
        ),
        (
            "rect one pixel past the last row",
            rect(0.0, 0.0, 240.0, 161.0),
            Code(OutOfBounds),
        ),
        (
            "rect with a negative corner",
            rect(-1.0, 0.0, 10.0, 10.0),
            Code(OutOfBounds),
        ),
        (
            "rect far outside",
            rect(600.0, 500.0, 800.0, 700.0),
            Code(OutOfBounds),
        ),
        (
            "rect with no width",
            rect(50.0, 40.0, 50.0, 90.0),
            Code(Degenerate),
        ),
        (
            "rect with no area",
            rect(5.0, 5.0, 5.0, 5.0),
            Code(Degenerate),
        ),
        (
            "rect with corners out of order",
            rect(90.0, 60.0, 30.0, 20.0),
            Code(Degenerate),
        ),
        (
            "rect whose width quantizes to nothing",
            rect(10.0, 10.0, 10.0004, 20.0),
            Code(Degenerate),
        ),
        (
            "point on the far corner",
            json!({ "type": "point", "x": 240, "y": 160 }),
            Valid,
        ),
        (
            "point left of the image",
            json!({ "type": "point", "x": -0.001, "y": 0 }),
            Code(OutOfBounds),
        ),
        ("line", points("line", &[(0.0, 0.0), (240.0, 160.0)]), Valid),
        (
            "line of no length",
            points("line", &[(5.0, 5.0), (5.0, 5.0)]),
            Valid,
        ),
        (
            "line leaving the image",
            points("line", &[(0.0, 0.0), (240.0, 160.5)]),
            Code(OutOfBounds),
        ),
        (
            "polyline of two points",
            points("polyline", &[(1.0, 1.0), (2.0, 2.0)]),
            Valid,
        ),
        (
            "polyline of one point",
            points("polyline", &[(1.0, 1.0)]),
            Code(TooFewPoints),
        ),
        (
            "polyline of no points",
            points("polyline", &[]),
            Code(TooFewPoints),
        ),
        (
            "polygon of three points",
            points("polygon", &[(1.0, 1.0), (20.0, 1.0), (1.0, 20.0)]),
            Valid,
        ),
        (
            "polygon of two points",
            points("polygon", &[(1.0, 1.0), (20.0, 1.0)]),
            Code(TooFewPoints),
        ),
        (
            "polygon with a vertex outside",
            points("polygon", &[(1.0, 1.0), (241.0, 1.0), (1.0, 20.0)]),
            Code(OutOfBounds),
        ),
        ("ellipse", ellipse(100.0, 80.0, 50.0, 30.0, 0.0), Valid),
        (
            "ellipse touching two edges",
            ellipse(50.0, 30.0, 50.0, 30.0, 0.0),
            Valid,
        ),
        (
            "ellipse crossing the left edge",
            ellipse(49.999, 30.0, 50.0, 30.0, 0.0),
            Code(OutOfBounds),
        ),
        (
            "ellipse turned to fit",
            ellipse(120.0, 80.0, 20.0, 100.0, 90.0),
            Valid,
        ),
        (
            "ellipse turned out of the image",
            ellipse(120.0, 80.0, 100.0, 20.0, 90.0),
            Code(OutOfBounds),
        ),
        (
            "ellipse with no first radius",
            ellipse(100.0, 80.0, 0.0, 30.0, 0.0),
            Code(Degenerate),
        ),
        (
            "ellipse with a negative radius",
            ellipse(100.0, 80.0, 50.0, -30.0, 0.0),
            Code(Degenerate),
        ),
        (
            "ellipse at 179.999 degrees",
            ellipse(100.0, 80.0, 5.0, 5.0, 179.999),
            Valid,
        ),
        (
            "ellipse at 180 degrees",
            ellipse(100.0, 80.0, 5.0, 5.0, 180.0),
            Code(BadAngle),
        ),
        (
            "ellipse at a negative angle",
            ellipse(100.0, 80.0, 5.0, 5.0, -1.0),
            Code(BadAngle),
        ),
        (
            "mask",
            mask(
                64,
                1,
                json!({ "0": { "0,0": "AAAA", "3,2": "AAECAwQ=" }, "2": { "1,1": "AA==" } }),
            ),
            Valid,
        ),
        (
            "mask of eight bits",
            mask(64, 8, json!({ "1": { "0,0": "AAAA" } })),
            Valid,
        ),
        (
            "mask with another tile size",
            mask(32, 1, json!({ "0": { "0,0": "AAAA" } })),
            Code(MaskLayout),
        ),
        (
            "mask with another depth",
            mask(64, 2, json!({ "0": { "0,0": "AAAA" } })),
            Code(MaskLayout),
        ),
        (
            "mask with no frame",
            mask(64, 1, json!({})),
            Code(MaskEmpty),
        ),
        (
            "mask with a frame without tiles",
            mask(64, 1, json!({ "0": {} })),
            Code(MaskEmpty),
        ),
        (
            "mask on a frame past the last",
            mask(64, 1, json!({ "3": { "0,0": "AAAA" } })),
            Code(FrameOutOfRange),
        ),
        (
            "mask tile right of the image",
            mask(64, 1, json!({ "0": { "4,0": "AAAA" } })),
            Code(MaskTileOutOfBounds),
        ),
        (
            "mask tile below the image",
            mask(64, 1, json!({ "0": { "0,3": "AAAA" } })),
            Code(MaskTileOutOfBounds),
        ),
        (
            "mask tile with an empty payload",
            mask(64, 1, json!({ "0": { "0,0": "" } })),
            Code(MaskPayload),
        ),
        (
            "mask tile that is not base64",
            mask(64, 1, json!({ "0": { "0,0": "not base64!!" } })),
            Code(MaskPayload),
        ),
        (
            "mask tile with a truncated payload",
            mask(64, 1, json!({ "0": { "0,0": "AAA" } })),
            Code(MaskPayload),
        ),
        (
            "mask tile with three padding characters",
            mask(64, 1, json!({ "0": { "0,0": "A===" } })),
            Code(MaskPayload),
        ),
        (
            "mask tile with padding in the middle",
            mask(64, 1, json!({ "0": { "0,0": "AA==AAAA" } })),
            Code(MaskPayload),
        ),
    ];
    for (name, value, expected) in cases {
        assert_outcome(name, geometry(value).validate(SIZE), expected);
    }

    // What JSON cannot carry but a Rust caller can build.
    for value in [f64::NAN, f64::INFINITY] {
        assert_outcome(
            "a number that is not finite",
            Geometry::Point { x: value, y: 1.0 }.validate(SIZE),
            Code(NonFinite),
        );
    }
}

/// Owner decision (EMBED parity amendment, 2026-10-05): a coordinate that is
/// negative or not an integer is clamped with a warning. It is rounded to
/// the nearest pixel first, a negative value becomes 0, a value past the
/// edge becomes the edge, and the row is reported.
#[test]
fn clamping_rounds_first_then_moves_onto_the_edge() {
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Geometry::Rect { x0, y0, x1, y1 };
    // (name, input, snap, output, rounded, moved)
    let cases: Vec<(&str, Geometry, Snap, Geometry, bool, bool)> = vec![
        (
            "already whole and inside",
            rect(20.0, 10.0, 80.0, 50.0),
            Snap::PixelEdges,
            rect(20.0, 10.0, 80.0, 50.0),
            false,
            false,
        ),
        (
            "on the far edge",
            rect(0.0, 0.0, 240.0, 160.0),
            Snap::PixelEdges,
            rect(0.0, 0.0, 240.0, 160.0),
            false,
            false,
        ),
        (
            "a negative value becomes 0",
            rect(20.0, -1.0, 80.0, 50.0),
            Snap::PixelEdges,
            rect(20.0, 0.0, 80.0, 50.0),
            false,
            true,
        ),
        (
            "a value past the edge becomes the edge",
            rect(20.0, 10.0, 241.0, 161.0),
            Snap::PixelEdges,
            rect(20.0, 10.0, 240.0, 160.0),
            false,
            true,
        ),
        (
            "a value that is not whole is rounded",
            rect(20.4, 10.5, 79.5, 49.6),
            Snap::PixelEdges,
            rect(20.0, 11.0, 80.0, 50.0),
            true,
            false,
        ),
        (
            "rounded first, then clamped",
            rect(-0.4, -0.6, 240.4, 160.6),
            Snap::PixelEdges,
            rect(0.0, 0.0, 240.0, 160.0),
            true,
            true,
        ),
        (
            "rounding alone brings it inside",
            rect(-0.4, 0.0, 240.4, 160.0),
            Snap::PixelEdges,
            rect(0.0, 0.0, 240.0, 160.0),
            true,
            false,
        ),
        (
            "a box far outside has no area left",
            rect(600.0, 500.0, 800.0, 700.0),
            Snap::PixelEdges,
            rect(240.0, 160.0, 240.0, 160.0),
            false,
            true,
        ),
        (
            "corners out of order stay out of order",
            rect(90.0, 60.0, -30.0, 20.0),
            Snap::PixelEdges,
            rect(90.0, 60.0, 0.0, 20.0),
            false,
            true,
        ),
        (
            "the model's own grid keeps decimals",
            rect(20.4004, 10.0, 80.0, 160.25),
            Snap::Quantum,
            rect(20.4, 10.0, 80.0, 160.0),
            true,
            true,
        ),
        (
            "every vertex of a polygon",
            Geometry::Polygon {
                points: vec![
                    Point { x: -5.0, y: 5.5 },
                    Point { x: 300.0, y: 5.0 },
                    Point { x: 100.0, y: 200.0 },
                ],
            },
            Snap::PixelEdges,
            Geometry::Polygon {
                points: vec![
                    Point { x: 0.0, y: 6.0 },
                    Point { x: 240.0, y: 5.0 },
                    Point { x: 100.0, y: 160.0 },
                ],
            },
            true,
            true,
        ),
        (
            "a point",
            Geometry::Point { x: 240.7, y: -3.0 },
            Snap::PixelEdges,
            Geometry::Point { x: 240.0, y: 0.0 },
            true,
            true,
        ),
        (
            "an ellipse is left alone",
            Geometry::Ellipse {
                cx: -10.0,
                cy: 80.5,
                rx: 50.0,
                ry: 30.0,
                angle: 0.0,
            },
            Snap::PixelEdges,
            Geometry::Ellipse {
                cx: -10.0,
                cy: 80.5,
                rx: 50.0,
                ry: 30.0,
                angle: 0.0,
            },
            false,
            false,
        ),
    ];
    for (name, input, snap, output, rounded, moved) in cases {
        let clamped = input.clamped(SIZE, snap);
        assert_eq!(clamped.geometry, output, "{name}");
        assert_eq!(clamped.rounded, rounded, "{name}: rounded");
        assert_eq!(clamped.moved, moved, "{name}: moved");
    }
}
