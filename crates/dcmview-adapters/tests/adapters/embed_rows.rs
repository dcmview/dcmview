//! EMBED ROI CSV as rows: the bytes written, and what a file is read as.

use super::support::{loaded, report_json, size, Pool};
use dcmview_adapters::embed::{check_header, read_rows, write_csv, EmbedRow};
use dcmview_adapters::limits::{CSV_READ_AHEAD_BYTES, MAX_CSV_ROW_BYTES, MAX_ROIS_PER_ROW};
use dcmview_adapters::{Bound, ImportError};
use serde_json::json;
use std::borrow::Cow;
use std::io::Read;

type Boxes = Vec<[u32; 4]>;
type Frames = Vec<Vec<u32>>;

/// Four loaded files. `link.dcm` names two of them.
fn pool() -> Pool {
    Pool(vec![
        loaded("a.dcm", "sop:1.2.3.1", size(160, 240, 1)),
        loaded("m.dcm", "sop:1.2.3.2", size(160, 240, 3)),
        loaded("link.dcm", "sop:1.2.3.3", size(16, 16, 1)),
        loaded("link.dcm", "sop:1.2.3.3", size(16, 16, 1)),
        loaded("b.dcm", "sop:1.2.3.4", size(160, 240, 1)),
    ])
}

fn read(csv: &str) -> Result<dcmview_adapters::EmbedRead, ImportError> {
    read_rows(&mut csv.as_bytes(), &pool(), &|| true)
}

#[test]
fn rows_are_written_sorted_by_path_with_the_released_quoting() {
    let frames = |lists: &[&[u32]]| lists.iter().map(|list| list.to_vec()).collect::<Vec<_>>();
    let rows: Vec<(&str, Boxes, Frames)> = vec![
        ("/data/single-frame.dcm", vec![[20, 30, 60, 90]], vec![]),
        (
            "/data/multiframe.dcm",
            vec![[10, 20, 50, 80], [100, 120, 160, 240]],
            frames(&[&[0, 1], &[2]]),
        ),
        ("/data/comma, name.dcm", vec![[1, 2, 3, 4]], frames(&[&[0]])),
        // Byte order: an upper-case letter stands before every lower-case one.
        ("/data/Zebra.dcm", vec![[1, 1, 2, 2]], frames(&[&[2, 0, 0]])),
        // Two rows with one path keep the order they were given in.
        ("/data/multiframe.dcm", vec![[7, 7, 8, 8]], vec![]),
        // A path that is a prefix of another stands first.
        (
            "/data/multiframe.dcm.bak",
            vec![[5, 6, 7, 8]],
            frames(&[&[]]),
        ),
    ];
    let mut out = Vec::new();
    write_csv(
        rows.iter().map(|(path, coords, frames)| EmbedRow {
            path: Cow::Borrowed(path),
            roi_coords: coords,
            roi_frames: frames,
        }),
        &mut out,
    )
    .expect("written");

    assert_eq!(
        String::from_utf8(out).expect("text"),
        concat!(
            "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n",
            "/data/Zebra.dcm,1,\"[[1,1,2,2]]\",\"[[2,0,0]]\"\n",
            "\"/data/comma, name.dcm\",1,\"[[1,2,3,4]]\",[[0]]\n",
            "/data/multiframe.dcm,2,\"[[10,20,50,80],[100,120,160,240]]\",\"[[0,1],[2]]\"\n",
            "/data/multiframe.dcm,1,\"[[7,7,8,8]]\",[]\n",
            "/data/multiframe.dcm.bak,1,\"[[5,6,7,8]]\",[[]]\n",
            "/data/single-frame.dcm,1,\"[[20,30,60,90]]\",[]\n",
        )
    );
}

/// The lenient import: a negative coordinate is read as 0 and one that is
/// not whole is rounded, each reported; a box past the edge, without area
/// or with its corners out of order is kept as read and reported; a row
/// that names no loaded file is counted and not parsed.
#[test]
fn matched_rows_are_read_leniently_and_everything_changed_or_odd_is_reported() {
    let csv = concat!(
        "empi_anon,ROI_coords,ROI_frames,side,num_ROI,anon_dicom_path\n",
        // Row 2: a negative value; values that are not whole (halves away
        // from zero); whole values written as decimals.
        "1,\"[[-1, 20, 50, 80], [20.5, 30.4, 59.5, 90.0], [2e1, 30.0, 60, 200]]\",[],L,3,a.dcm\n",
        // Row 3: what the import never checked, and still does not.
        "2,\"[[0,0,161,241],[500,600,700,800],[40,50,40,90],[5,5,5,5],[60,90,20,30]]\",\"[[0],[],[2,0,0],[0],[0]]\",R, 5 ,  m.dcm \n",
        // Row 4 names no loaded file: nothing of it is read.
        "3,not-json,also-not-json,R,many,nowhere.dcm\n",
        // Row 5 names two loaded files.
        "4,\"[[1,2,3,4]]\",[],L,1,link.dcm\n",
        // Row 6: one box with both kinds of correction.
        "5,\"[[-0.4, -7.9, 2.5, 4]]\",[],L,1,b.dcm\n",
    );

    let read = read(csv).expect("read");

    let files: Vec<(usize, u64, Boxes, Frames)> = read
        .files
        .iter()
        .map(|file| {
            (
                file.slot,
                file.row,
                file.rois.roi_coords.clone(),
                file.rois.roi_frames.clone(),
            )
        })
        .collect();
    assert_eq!(
        files,
        vec![
            (
                0,
                2,
                // The last box reaches column 200 of 240: wider than the
                // image is high, and inside it.
                vec![[0, 20, 50, 80], [21, 30, 60, 90], [20, 30, 60, 200]],
                vec![]
            ),
            (
                1,
                3,
                vec![
                    [0, 0, 161, 241],
                    [500, 600, 700, 800],
                    [40, 50, 40, 90],
                    [5, 5, 5, 5],
                    [60, 90, 20, 30]
                ],
                vec![vec![0], vec![], vec![2, 0, 0], vec![0], vec![0]]
            ),
            (2, 5, vec![[1, 2, 3, 4]], vec![]),
            (3, 5, vec![[1, 2, 3, 4]], vec![]),
            (4, 6, vec![[0, 0, 3, 4]], vec![]),
        ]
    );
    assert_eq!(
        report_json(&read.report),
        json!({
            "adapter": { "id": "embed", "version": 1 },
            "direction": "import",
            "counts": {
                "files": 5, "annotations_in": 10, "annotations_out": 11,
                "labels_in": 0, "labels_out": 0, "unmatched": 1, "refused": 0,
            },
            "losses": [],
            "warnings": [
                { "code": "negative_coordinate", "count": 2,
                  "rows": [{ "row": 2, "roi": 0 }, { "row": 6, "roi": 0 }] },
                { "code": "non_integer_coordinate", "count": 2,
                  "rows": [{ "row": 2, "roi": 1 }, { "row": 6, "roi": 0 }] },
                { "code": "out_of_range", "count": 2,
                  "rows": [{ "row": 3, "roi": 0 }, { "row": 3, "roi": 1 }] },
                { "code": "frames_empty", "count": 1, "rows": [{ "row": 3, "roi": 1 }] },
                { "code": "zero_area", "count": 2,
                  "rows": [{ "row": 3, "roi": 2 }, { "row": 3, "roi": 3 }] },
                { "code": "corners_out_of_order", "count": 1, "rows": [{ "row": 3, "roi": 4 }] },
            ],
            "matched_by": { "key": 0, "sop_instance_uid": 0, "path": 4 },
        })
    );
}

/// A report is bounded whatever the file holds: an entry counts every
/// occurrence and names the first sixteen places.
#[test]
fn a_report_names_the_first_places_and_counts_them_all() {
    let boxes = vec!["[-1,2,3,4]"; 20].join(",");
    let csv = format!("anon_dicom_path,ROI_coords\na.dcm,\"[{boxes}]\"\nb.dcm,\"[[0,0,-2,0]]\"\n");
    let report = report_json(&read(&csv).expect("read").report);
    let negative = &report["warnings"][0];
    assert_eq!(negative["code"], "negative_coordinate");
    assert_eq!(negative["count"], 21);
    let rows = negative["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), dcmview_adapters::limits::MAX_REPORT_ROWS);
    assert_eq!(rows[0], json!({ "row": 2, "roi": 0 }));
    assert_eq!(rows[15], json!({ "row": 2, "roi": 15 }));
    // The other kind of warning is an entry of its own, after it.
    assert_eq!(report["warnings"][1]["code"], "zero_area");
    assert_eq!(report["warnings"][1]["count"], 1);
}

#[test]
fn only_the_two_required_columns_are_needed() {
    let read = read("anon_dicom_path,ROI_coords\na.dcm,\"[[20,30,60,90],[0,0,160,240]]\"\n")
        .expect("read");
    assert_eq!(read.files.len(), 1);
    assert_eq!(
        read.files[0].rois.roi_coords,
        vec![[20, 30, 60, 90], [0, 0, 160, 240]]
    );
    assert!(read.files[0].rois.roi_frames.is_empty());
    assert!(read.report.warnings.is_empty());

    assert_eq!(
        check_header(&mut "side,ROI_coords,anon_dicom_path\nx".as_bytes()),
        Ok(())
    );
    assert_eq!(
        check_header(&mut "anon_dicom_path,num_ROI,ROI_frames\n".as_bytes()),
        Err(ImportError::MissingColumn {
            column: "ROI_coords"
        })
    );
    assert_eq!(
        check_header(&mut "ROI_coords,num_ROI\n".as_bytes()),
        Err(ImportError::MissingColumn {
            column: "anon_dicom_path"
        })
    );
    assert_eq!(
        check_header(&mut "".as_bytes()),
        Err(ImportError::MissingColumn {
            column: "anon_dicom_path"
        })
    );
}

/// Every way a matched row fails the whole import, with the row it names.
#[test]
fn an_invalid_matched_row_fails_the_whole_import() {
    const HEADER: &str = "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n";
    const GOOD: &str = "a.dcm,1,\"[[20,30,60,90]]\",[]\n";
    let cases: Vec<(&str, &str, ImportError)> = vec![
        (
            "a frame index equal to the frame count",
            "m.dcm,1,\"[[10,20,50,80]]\",[[3]]\n",
            ImportError::FrameOutOfRange {
                row: 3,
                roi: 0,
                frame: 3,
                frame_count: 3,
            },
        ),
        (
            "a second row for the same file",
            "m.dcm,1,\"[[10,20,50,80]]\",[]\na.dcm,1,\"[[1,2,3,4]]\",[]\n",
            ImportError::DuplicatePath {
                row: 4,
                previous_row: 2,
            },
        ),
        (
            "num_ROI is not the number of boxes",
            "m.dcm,2,\"[[10,20,50,80]]\",[]\n",
            ImportError::NumRoiMismatch {
                row: 3,
                coords: 1,
                num_roi: 2,
            },
        ),
        (
            "num_ROI is not a number",
            "m.dcm,two,\"[[10,20,50,80]]\",[]\n",
            ImportError::NumRoi { row: 3 },
        ),
        (
            "fewer frame lists than boxes",
            "m.dcm,2,\"[[10,20,50,80],[100,120,160,240]]\",[[0]]\n",
            ImportError::RoiFramesLength {
                row: 3,
                frames: 1,
                num_roi: 2,
            },
        ),
        (
            "frames that are not lists of whole numbers",
            "m.dcm,1,\"[[10,20,50,80]]\",[[0.5]]\n",
            ImportError::RoiFrames { row: 3 },
        ),
        (
            "boxes that are not JSON",
            "m.dcm,1,not-json,[]\n",
            ImportError::RoiCoords { row: 3 },
        ),
        (
            "a coordinate that is not a number",
            "m.dcm,1,\"[[10,\"\"20\"\",50,80]]\",[]\n",
            ImportError::RoiCoords { row: 3 },
        ),
        (
            "a box of five numbers",
            "m.dcm,2,\"[[10,20,50,80],[1,2,3,4,5]]\",[]\n",
            ImportError::RoiCoordsEntry { row: 3, roi: 1 },
        ),
        (
            "a coordinate past what a box can hold",
            "m.dcm,1,\"[[10,20,50,4294967295.5]]\",[]\n",
            ImportError::CoordinateTooLarge { row: 3, roi: 0 },
        ),
        (
            "an empty path",
            "  ,1,\"[[10,20,50,80]]\",[]\n",
            ImportError::EmptyPath { row: 3 },
        ),
        (
            "a record with a field too few",
            "m.dcm,1,\"[[10,20,50,80]]\"\n",
            ImportError::CsvRow { row: 3 },
        ),
    ];
    for (what, rows, expected) in cases {
        let csv = format!("{HEADER}{GOOD}{rows}");
        assert_eq!(read(&csv).err(), Some(expected), "{what}");
    }

    // The largest coordinate a box can hold is read.
    let most = read(&format!(
        "{HEADER}a.dcm,1,\"[[0,0,4294967295,4294967295.4]]\",[]\n"
    ))
    .expect("the largest coordinate");
    assert_eq!(
        most.files[0].rois.roi_coords,
        vec![[0, 0, u32::MAX, u32::MAX]]
    );
}

#[test]
fn an_import_that_is_told_to_stop_hands_nothing_on() {
    let csv = "anon_dicom_path,ROI_coords\na.dcm,\"[[1,2,3,4]]\"\nb.dcm,\"[[1,2,3,4]]\"\n";
    let asked = std::cell::Cell::new(0);
    let result = read_rows(&mut csv.as_bytes(), &pool(), &|| {
        asked.set(asked.get() + 1);
        asked.get() < 2
    });
    assert_eq!(result.err(), Some(ImportError::Cancelled));
}

/// An input that never ends, counted: what a reader was given of it.
struct Endless {
    start: std::io::Cursor<Vec<u8>>,
    fill: u8,
    given: u64,
}

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = self.start.read(buffer)?;
        if read == 0 {
            buffer.fill(self.fill);
            read = buffer.len();
        }
        self.given += read as u64;
        Ok(read)
    }
}

/// A file is not read into memory to find out that a record never ends.
#[test]
fn a_record_past_its_bound_is_refused_before_it_is_read_whole() {
    let too_large = ImportError::TooLarge {
        bound: Bound::CsvRowBytes,
        limit: MAX_CSV_ROW_BYTES,
    };
    let block = 64 * 1024;
    // A quoted field that is never closed, in a row and in the header.
    for start in [
        "anon_dicom_path,ROI_coords\na.dcm,\"[[1,2,3,4]]\"\nb.dcm,\"",
        "anon_dicom_path,ROI_coords,\"",
    ] {
        let mut input = Endless {
            start: std::io::Cursor::new(start.as_bytes().to_vec()),
            fill: b'x',
            given: 0,
        };
        assert_eq!(
            read_rows(&mut input, &pool(), &|| true).err(),
            Some(too_large.clone())
        );
        assert!(
            input.given <= MAX_CSV_ROW_BYTES + CSV_READ_AHEAD_BYTES + start.len() as u64 + block,
            "{} bytes were read",
            input.given
        );
    }

    // Exactly at the bound a record is read; one byte more and it is not,
    // also when its path names no file.
    let row = |bytes: u64| {
        let fixed = "nowhere.dcm,[],\n".len() as u64;
        format!(
            "anon_dicom_path,ROI_coords,padding\nnowhere.dcm,[],{}\n",
            "p".repeat((bytes - fixed) as usize)
        )
    };
    let fits = read(&row(MAX_CSV_ROW_BYTES)).expect("a record at the bound");
    assert_eq!(fits.report.counts.unmatched, 1);
    assert_eq!(read(&row(MAX_CSV_ROW_BYTES + 1)).err(), Some(too_large));
}

#[test]
fn a_row_holds_a_bounded_number_of_boxes_and_frames() {
    let boxes = |count: usize| {
        format!(
            "anon_dicom_path,ROI_coords\na.dcm,\"[{}]\"\n",
            vec!["[1,2,3,4]"; count].join(",")
        )
    };
    assert_eq!(
        read(&boxes(MAX_ROIS_PER_ROW)).expect("at the bound").files[0]
            .rois
            .roi_coords
            .len(),
        MAX_ROIS_PER_ROW
    );
    assert_eq!(
        read(&boxes(MAX_ROIS_PER_ROW + 1)).err(),
        Some(ImportError::TooLarge {
            bound: Bound::RoisPerRow,
            limit: MAX_ROIS_PER_ROW as u64,
        })
    );

    let frames = |count: usize| {
        format!(
            "anon_dicom_path,ROI_coords,ROI_frames\nm.dcm,\"[[1,2,3,4]]\",\"[[{}]]\"\n",
            vec!["0"; count].join(",")
        )
    };
    let limit = dcmview_adapters::limits::MAX_FRAMES_PER_ROI;
    assert_eq!(
        read(&frames(limit)).expect("at the bound").files[0]
            .rois
            .roi_frames[0]
            .len(),
        limit
    );
    assert_eq!(
        read(&frames(limit + 1)).err(),
        Some(ImportError::TooLarge {
            bound: Bound::FramesPerRoi,
            limit: limit as u64,
        })
    );

    // A count the file states is compared, never allocated from.
    let stated = "anon_dicom_path,num_ROI,ROI_coords\na.dcm,18446744073709551615,\"[[1,2,3,4]]\"\n";
    assert_eq!(
        read(stated).err(),
        Some(ImportError::NumRoiMismatch {
            row: 2,
            coords: 1,
            num_roi: usize::MAX,
        })
    );
}
