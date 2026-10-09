//! The metadata tree of raster image files through the HTTP API: what
//! `/tags` serves for each format, how `/tags/select` addresses a node, and
//! what a masked session shows (`docs/design/image-formats.md` section 8
//! and its re-baseline amendment).
//!
//! Every file is written into a temporary directory and listed by the
//! production loader, so what is read is what a client of the server reads.
//! What the tree holds for each kind of entry, and what reading it may cost,
//! is in `tests/raster_cost/tags.rs`.

use super::raster_discovery::{options, scan_dir, scan_into, Scan};
use super::raster_tag_files::{self as tagged, rgb_page_samples, rgb_page_tags, rows, Block, V};
use super::{raster_files, support};
use dcmview::api::contracts::endpoints;
use dcmview::loader::FormatSelection;
use dcmview::masking::Masker;
use dcmview::server::FileRegistry;
use serde_json::{json, Value};
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

async fn tags(scan: &Scan, name: &str) -> Value {
    let response = scan
        .server
        .get(&format!("/api/file/{}/tags", scan.index(name)))
        .await;
    response.assert_status_ok();
    response.json()
}

/// `GET /tags/select` for `name` with the given query pairs.
async fn select(scan: &Scan, name: &str, query: &[(&str, &str)]) -> axum_test::TestResponse {
    let mut request = scan
        .server
        .get(&format!("/api/file/{}/tags/select", scan.index(name)));
    for (key, value) in query {
        request = request.add_query_param(key, value);
    }
    request.await
}

fn has_row(rows: &[String], row: &str) -> bool {
    rows.iter().any(|found| found == row)
}

#[tokio::test]
async fn every_raster_format_serves_its_metadata_tree() {
    let dir = tempdir().expect("temp dir");
    let planted = tagged::planted_files();
    for file in &planted {
        fs::write(dir.path().join(file.name), &file.bytes).expect("write raster");
    }
    // A PNG under a JPEG's name, a TIFF with a page that is not a frame,
    // and an animation.
    fs::write(dir.path().join("renamed.jpg"), &planted[0].bytes).expect("write PNG");
    let mut wider = rgb_page_tags();
    wider[0] = (256, V::Long(vec![3]));
    let stack = tagged::tiff_block(&Block {
        data: rgb_page_samples(),
        ifd0: rgb_page_tags(),
        next: vec![wider],
        ..Block::default()
    })
    .bytes;
    fs::write(dir.path().join("stack.tif"), &stack).expect("write TIFF");
    fs::write(
        dir.path().join("moving.webp"),
        raster_files::animated_webp(),
    )
    .expect("write WebP");
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.not_loaded(), []);

    // (file, rows its tree holds)
    let size = |index: usize| format!("File | Size |  | {}", planted[index].bytes.len());
    let cases: [(&str, Vec<String>); 4] = [
        (
            "planted.png",
            vec![
                "File | Format |  | \"png\"".into(),
                size(0),
                "File | Extension |  | \"png\"".into(),
                "File | Pages |  | 1".into(),
                "File | Frames |  | 1".into(),
                "PNG:IHDR | Width |  | 2".into(),
                "PNG:tIME | Time |  | \"1986-05-04 03:02:01\"".into(),
                "EXIF/GPS/0x0002 | GPSLatitude | RATIONAL | \"48123/1000, 0/1, 0/1\"".into(),
                "ICC | Class |  | \"zqPc\"".into(),
            ],
        ),
        (
            "planted.jpg",
            vec![
                "File | Format |  | \"jpeg\"".into(),
                size(1),
                "File | Extension |  | \"jpg\"".into(),
                "JPEG:JFIF | Version |  | \"1.01\"".into(),
                "JPEG:SOF0 | Width |  | 8".into(),
                "EXIF/Exif/0x9003 | DateTimeOriginal | ASCII | \"1988:07:06 05:04:03\"".into(),
            ],
        ),
        (
            "planted.tif",
            vec![
                "File | Format |  | \"tiff\"".into(),
                size(2),
                "File | Pages |  | 2".into(),
                "File | Frames |  | 2".into(),
                "TIFF:page 0/0x0100 | ImageWidth | LONG | 2".into(),
                "TIFF:page 0/GPS/0x001D | GPSDateStamp | ASCII | \"1989:08:07\"".into(),
                "TIFF:page 1/0x0100 | ImageWidth | LONG | 2".into(),
            ],
        ),
        (
            "planted.webp",
            vec![
                "File | Format |  | \"webp\"".into(),
                size(3),
                "WEBP:VP8X | CanvasWidth |  | 8".into(),
                "EXIF/IFD0/0x0132 | DateTime | ASCII | \"1987:06:05 04:03:02\"".into(),
            ],
        ),
    ];
    for (name, expected) in &cases {
        let tree = tags(&scan, name).await;
        let rows = rows(&tree);
        for row in expected {
            assert!(has_row(&rows, row), "{name}: no row {row:?} in {rows:#?}");
        }
        assert!(
            !rows.iter().any(|row| row.starts_with("Note |")),
            "{name}: a well-formed file has no notes: {rows:#?}"
        );
        // Asked again, the same tree.
        assert_eq!(tags(&scan, name).await, tree, "{name}");
    }

    // What the catalog knows without reading is shown with the tree.
    let renamed = rows(&tags(&scan, "renamed.jpg").await);
    assert!(
        has_row(&renamed, "File | Format |  | \"png\""),
        "{renamed:#?}"
    );
    assert!(
        has_row(&renamed, "File | Extension |  | \"jpg\""),
        "{renamed:#?}"
    );
    assert_eq!(
        renamed
            .iter()
            .filter(|row| row.starts_with("Note |"))
            .count(),
        1,
        "a name that suggests another format is noted: {renamed:#?}"
    );
    let stack = rows(&tags(&scan, "stack.tif").await);
    for row in [
        "File | Pages |  | 2",
        "File | Frames |  | 1",
        "File | ExcludedPages |  | 1",
        "File | ExcludedPage |  | \"page 1: width\"",
        "TIFF:page 1/0x0100 | ImageWidth | LONG | 3",
    ] {
        assert!(has_row(&stack, row), "no row {row:?} in {stack:#?}");
    }
    let moving = rows(&tags(&scan, "moving.webp").await);
    assert_eq!(
        moving
            .iter()
            .filter(|row| row.starts_with("Note |"))
            .count(),
        1,
        "an animation is noted: {moving:#?}"
    );

    // A file that is gone when its metadata is first asked for.
    fs::remove_file(dir.path().join("stack.tif")).expect("remove TIFF");
    assert!(
        !rows(&tags(&scan, "stack.tif").await).is_empty(),
        "served from the cache"
    );
    fs::remove_file(dir.path().join("moving.webp")).expect("remove WebP");
    assert!(
        !rows(&tags(&scan, "moving.webp").await).is_empty(),
        "served from the cache"
    );
    let dir2 = tempdir().expect("temp dir");
    fs::write(dir2.path().join("gone.png"), &planted[0].bytes).expect("write PNG");
    let scan2 = scan_dir(dir2.path()).await;
    fs::remove_file(dir2.path().join("gone.png")).expect("remove PNG");
    for endpoint in [&endpoints::FILE_TAGS, &endpoints::FILE_TAG_SELECT] {
        let response =
            support::endpoint_request(&scan2.server, endpoint, &scan2.index("gone.png")).await;
        assert_eq!(response.status_code().as_u16(), 404, "{}", endpoint.id);
        let body: Value = response.json();
        assert!(
            body["code"].is_string() && body["error"].is_string(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_node_of_a_raster_tree_is_selected_by_its_path() {
    let dir = tempdir().expect("temp dir");
    for file in [tagged::planted_png(), tagged::planted_tiff()] {
        fs::write(dir.path().join(file.name), &file.bytes).expect("write raster");
    }
    let scan = scan_dir(dir.path()).await;
    let png = tags(&scan, "planted.png").await;
    let tiff = tags(&scan, "planted.tif").await;

    /// The node at `path` of `tree`: indices at each level.
    fn node<'a>(tree: &'a Value, path: &[usize]) -> &'a Value {
        let mut node = &tree[path[0]];
        for index in &path[1..] {
            node = &node["value"]["items"][0][*index];
        }
        node
    }
    let position = |nodes: &Value, tag: &str, keyword: &str, nth: usize| -> usize {
        nodes
            .as_array()
            .expect("nodes")
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                node["tag"] == tag && (keyword.is_empty() || node["keyword"] == keyword)
            })
            .nth(nth)
            .unwrap_or_else(|| panic!("no {tag}.{keyword}[{nth}]"))
            .0
    };
    let children = |node: &Value| node["value"]["items"][0].clone();

    let exif = position(&png, "EXIF", "", 0);
    let exif_children = children(&png[exif]);
    let gps = position(&exif_children, "GPS", "", 0);
    let ifd0 = position(&exif_children, "IFD0", "", 0);
    let latitude = position(&children(&exif_children[gps]), "0x0002", "", 0);
    let page1 = position(&tiff, "TIFF:page 1", "", 0);
    let description = position(&children(&tiff[page1]), "0x010E", "", 0);

    // (file, selector, the node of the tree it names)
    let found = [
        (
            "planted.png",
            "PNG:IHDR.Width",
            node(&png, &[position(&png, "PNG:IHDR", "Width", 0)]),
        ),
        (
            "planted.png",
            "PNG:IHDR",
            node(&png, &[position(&png, "PNG:IHDR", "", 0)]),
        ),
        (
            "planted.png",
            "PNG:IHDR[1]",
            node(&png, &[position(&png, "PNG:IHDR", "", 1)]),
        ),
        (
            "planted.png",
            " PNG:IHDR.Height[0] ",
            node(&png, &[position(&png, "PNG:IHDR", "Height", 0)]),
        ),
        (
            "planted.png",
            "PNG:tEXt[1]",
            node(&png, &[position(&png, "PNG:tEXt", "", 1)]),
        ),
        (
            "planted.png",
            "PNG:tEXt.Text[1]",
            node(&png, &[position(&png, "PNG:tEXt", "", 1)]),
        ),
        (
            "planted.png",
            "File.Size",
            node(&png, &[position(&png, "File", "Size", 0)]),
        ),
        (
            "planted.png",
            "EXIF/GPS/0x0002",
            node(&png, &[exif, gps, latitude]),
        ),
        (
            "planted.png",
            "/EXIF/GPS/0x0002.GPSLatitude/",
            node(&png, &[exif, gps, latitude]),
        ),
        ("planted.png", "EXIF/IFD0", node(&png, &[exif, ifd0])),
        ("planted.png", "EXIF", node(&png, &[exif])),
        (
            "planted.tif",
            "TIFF:page 1/0x010E",
            node(&tiff, &[page1, description]),
        ),
    ];
    for (name, selector, expected) in found {
        let response = select(&scan, name, &[("path", selector)]).await;
        assert_eq!(
            response.status_code().as_u16(),
            200,
            "{selector}: {}",
            response.text()
        );
        assert_eq!(&response.json::<Value>(), expected, "{selector}");
    }
    assert_eq!(node(&png, &[exif, gps, latitude])["keyword"], "GPSLatitude");
    assert_eq!(
        node(&tiff, &[page1, description])["keyword"],
        "ImageDescription"
    );

    // A group's items are paged like a sequence's; it has one.
    let paged = select(&scan, "planted.png", &[("path", "EXIF"), ("offset", "1")]).await;
    assert_eq!(
        paged.json::<Value>()["value"],
        json!({ "type": "sequence", "items": [], "truncated": true, "total": 1 })
    );
    let whole = select(
        &scan,
        "planted.png",
        &[("path", "EXIF"), ("offset", "0"), ("limit", "256")],
    )
    .await;
    assert_eq!(&whole.json::<Value>(), node(&png, &[exif]));

    // What names nothing is a bad request, never a server error.
    let long = "PNG:IHDR.".to_string() + &"W".repeat(300);
    let refused: [&[(&str, &str)]; 20] = [
        &[("path", "")],
        &[("path", "/")],
        &[("path", "NoSuchPart")],
        &[("path", "EXIF/NoSuchDirectory")],
        &[("path", "EXIF/GPS/0xFFFF")],
        &[("path", "PNG:IHDR.NoSuchField")],
        &[("path", "PNG:IHDR.Width[1]")],
        &[("path", "PNG:IHDR[7]")],
        &[("path", "PNG:IHDR[x]")],
        &[("path", "PNG:IHDR[-1]")],
        &[("path", "PNG:IHDR[18446744073709551616]")],
        &[("path", "PNG:IHDR]")],
        &[("path", "PNG:IHDR.Width/0x0001")],
        &[("path", "EXIF/IFD0/0x010F/0x0001")],
        &[("path", "EXIF/GPS/0x0002/0")],
        &[("path", "(0010,0010)")],
        &[("path", &long)],
        &[("path", "PNG:IHDR.Width"), ("offset", "1")],
        &[("path", "EXIF"), ("limit", "0")],
        &[("path", "EXIF"), ("limit", "257")],
    ];
    for query in refused {
        let response = select(&scan, "planted.png", query).await;
        assert_eq!(
            response.status_code().as_u16(),
            400,
            "{query:?}: {}",
            response.text()
        );
        let body: Value = response.json();
        assert!(
            body["code"].is_string() && body["error"].is_string(),
            "{query:?}: {body}"
        );
    }
    assert_eq!(
        select(&scan, "planted.png", &[])
            .await
            .status_code()
            .as_u16(),
        400,
        "a selection needs a path"
    );
}

/// A selector for every node of `nodes`, by tag and occurrence.
fn selectors(nodes: &Value, prefix: &str, out: &mut Vec<String>) {
    let nodes = nodes.as_array().expect("nodes");
    for (index, node) in nodes.iter().enumerate() {
        let tag = node["tag"].as_str().expect("tag");
        let nth = nodes[..index]
            .iter()
            .filter(|earlier| earlier["tag"] == tag)
            .count();
        let path = format!("{prefix}{tag}[{nth}]");
        if let Some(items) = node["value"]["items"].as_array() {
            for item in items {
                selectors(item, &format!("{path}/"), out);
            }
        }
        out.push(path);
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Every place a raster file can say who made it, where, when and with what
/// holds a string found nowhere else. An unmasked session shows each one
/// the tree has a value for; a masked session shows none of them, in any
/// response of any endpoint.
#[tokio::test]
async fn a_masked_session_shows_no_text_of_a_raster_file() {
    let dir = tempdir().expect("temp dir");
    let planted = tagged::planted_files();
    for file in &planted {
        fs::write(dir.path().join(file.name), &file.bytes).expect("write raster");
    }
    let everything = planted
        .iter()
        .flat_map(|file| file.planted.iter())
        .collect::<Vec<_>>();
    assert!(
        everything.len() >= 140,
        "{} strings are planted",
        everything.len()
    );

    let open = scan_dir(dir.path()).await;
    let mut paths = Vec::new();
    for file in &planted {
        let tree = tags(&open, file.name).await;
        let text = tree.to_string();
        for planted in &file.planted {
            assert_eq!(
                text.contains(&planted.text),
                planted.shown,
                "{}: {} ({:?}) in an unmasked tree",
                file.name,
                planted.place,
                planted.text
            );
        }
        let mut selectors_of_file = Vec::new();
        selectors(&tree, "", &mut selectors_of_file);
        paths.push(selectors_of_file);
    }

    let masked = scan_into(
        &[dir.path().to_path_buf()],
        options(FormatSelection::all(), &[]),
        FileRegistry::masked(Arc::new(Masker::new())),
    )
    .await;
    let assert_clean = |what: &str, body: &[u8]| {
        for planted in &everything {
            assert!(
                !contains(body, &planted.text),
                "{what} shows {} ({:?}) in a masked session",
                planted.place,
                planted.text
            );
        }
    };
    assert_clean("the catalog", masked.catalog.to_string().as_bytes());
    for (file, paths) in planted.iter().zip(&paths) {
        let index = masked.index(file.name);
        // Every endpoint of the contract, whatever it answers. (A colour
        // frame still carries the file's ICC profile to the browser, as a
        // DICOM colour frame does: that is pixel data's colour, compressed
        // inside the image, and nothing the viewer shows as text.)
        for endpoint in endpoints::ALL {
            let response = support::endpoint_request(&masked.server, endpoint, &index).await;
            assert_clean(
                &format!("{} {}", file.name, endpoint.id),
                response.as_bytes(),
            );
        }
        // Every node of the tree, selected one by one.
        for path in paths {
            let response = select(&masked, file.name, &[("path", path)]).await;
            assert_eq!(response.status_code().as_u16(), 200, "{} {path}", file.name);
            assert_clean(
                &format!("{} selected {path}", file.name),
                response.as_bytes(),
            );
        }
    }

    // What a masked session does show: the tree's shape, and of its values
    // only those that describe the pixel grid and its encoding.
    let shown = |tree: &Value| -> Vec<String> {
        rows(tree)
            .into_iter()
            .filter(|row| !row.ends_with("| \"[masked]\"") && !row.ends_with("| {"))
            .collect()
    };
    let size = |index: usize| format!("File | Size |  | {}", planted[index].bytes.len());
    let png = tags(&masked, "planted.png").await;
    assert_eq!(
        shown(&png),
        [
            "File | Format |  | \"png\"",
            &size(0),
            "File | Pages |  | 1",
            "File | Frames |  | 1",
            "PNG:IHDR | Width |  | 2",
            "PNG:IHDR | Height |  | 2",
            "PNG:IHDR | BitDepth |  | 8",
            "PNG:IHDR | ColorType |  | 2",
            "PNG:IHDR | Compression |  | 0",
            "PNG:IHDR | Filter |  | 0",
            "PNG:IHDR | Interlace |  | 0",
            "PNG:sBIT | SignificantBits |  | [8, 8, 8]",
            "PNG:pHYs | PixelsPerUnitX |  | 2835",
            "PNG:pHYs | PixelsPerUnitY |  | 2835",
            "PNG:pHYs | Unit |  | 1",
            "EXIF/IFD0/0x8769 | ExifIFD | LONG | 472",
            "EXIF/IFD0/0x8825 | GPSIFD | LONG | 750",
            "EXIF/Exif/0xA005 | InteropIFD | LONG | 902",
            "ICC | Size |  | 172",
            "ICC | Version |  | \"2.1.0\"",
        ]
    );
    // The tree keeps its shape: every node is still there, masked or not.
    assert_eq!(
        rows(&png).len(),
        rows(&tags(&open, "planted.png").await).len()
    );
    assert!(has_row(&rows(&png), "PNG:tEXt | Text |  | \"[masked]\""));
    assert!(has_row(
        &rows(&png),
        "EXIF/GPS/0x0002 | GPSLatitude | RATIONAL | \"[masked]\""
    ));
    assert!(has_row(&rows(&png), "File | Extension |  | \"[masked]\""));

    assert_eq!(
        shown(&tags(&masked, "planted.jpg").await),
        [
            "File | Format |  | \"jpeg\"",
            &size(1),
            "File | Pages |  | 1",
            "File | Frames |  | 1",
            "JPEG:JFIF | Version |  | \"1.01\"",
            "JPEG:JFIF | Units |  | 1",
            "JPEG:JFIF | XDensity |  | 72",
            "JPEG:JFIF | YDensity |  | 72",
            "JPEG:SOF0 | Precision |  | 8",
            "JPEG:SOF0 | Height |  | 8",
            "JPEG:SOF0 | Width |  | 8",
            "JPEG:SOF0 | Components |  | 3",
            "EXIF/IFD0/0x8769 | ExifIFD | LONG | 472",
            "EXIF/IFD0/0x8825 | GPSIFD | LONG | 750",
            "EXIF/Exif/0xA005 | InteropIFD | LONG | 902",
            "ICC | Size |  | 172",
            "ICC | Version |  | \"2.1.0\"",
        ]
    );
    let page = |page: usize| -> Vec<String> {
        [
            "0x0100 | ImageWidth | LONG | 2",
            "0x0101 | ImageLength | LONG | 2",
            "0x0102 | BitsPerSample | SHORT | [8, 8, 8]",
            "0x0103 | Compression | SHORT | 1",
            "0x0106 | PhotometricInterpretation | SHORT | 2",
            "0x0111 | StripOffsets | LONG | 8",
            "0x0115 | SamplesPerPixel | SHORT | 3",
            "0x0116 | RowsPerStrip | LONG | 2",
            "0x0117 | StripByteCounts | LONG | 12",
        ]
        .iter()
        .map(|row| format!("TIFF:page {page}/{row}"))
        .collect()
    };
    let own = |rows: &[&str]| rows.iter().map(|row| row.to_string()).collect::<Vec<_>>();
    assert_eq!(
        shown(&tags(&masked, "planted.tif").await),
        [
            own(&[
                "File | Format |  | \"tiff\"",
                &size(2),
                "File | Pages |  | 2",
                "File | Frames |  | 2",
            ]),
            page(0),
            own(&[
                "TIFF:page 0/0x8769 | ExifIFD | LONG | 880",
                "TIFF:page 0/0x8825 | GPSIFD | LONG | 1158",
                "TIFF:page 0/Exif/0xA005 | InteropIFD | LONG | 1310",
            ]),
            page(1),
            own(&["ICC | Size |  | 172", "ICC | Version |  | \"2.1.0\""]),
        ]
        .concat()
    );
    assert_eq!(
        shown(&tags(&masked, "planted.webp").await),
        [
            "File | Format |  | \"webp\"",
            &size(3),
            "File | Pages |  | 1",
            "File | Frames |  | 1",
            "WEBP:VP8X | Flags |  | 44",
            "WEBP:VP8X | CanvasWidth |  | 8",
            "WEBP:VP8X | CanvasHeight |  | 8",
            "EXIF/IFD0/0x8769 | ExifIFD | LONG | 496",
            "EXIF/IFD0/0x8825 | GPSIFD | LONG | 786",
            "EXIF/Exif/0xA005 | InteropIFD | LONG | 938",
            "ICC | Size |  | 200",
            "ICC | Version |  | \"4.3.0\"",
        ]
    );
}
