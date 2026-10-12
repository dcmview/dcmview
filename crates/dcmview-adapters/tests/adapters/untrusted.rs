//! An imported file is input nobody vouches for: whatever it holds, an
//! import ends with a result or an error, and neither says anything the
//! file said.

use super::support::{
    embed_golden_inputs, fixture_document_text, key, loaded, report_json, size, Pool, Session,
};
use dcmview_adapters::embed::read_rows;
use dcmview_adapters::{EmbedAdapter, ImportAdapter, Limits, NativeJson, Registry};
use dcmview_annotation::Document;

/// A small generator with a fixed seed, so every run mutates the same way.
struct Mutations(u64);

impl Mutations {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    /// `input` with one change: a byte replaced, a run removed or
    /// repeated, the end cut off, or a piece of hostile text put in.
    fn mutate(&mut self, input: &[u8]) -> Vec<u8> {
        const PIECES: [&[u8]; 12] = [
            b"\"",
            b",",
            b"\n",
            b"[[",
            b"]]",
            b"{",
            b"}",
            b"-1",
            b"1e999",
            b"4294967296",
            b"\xff\xfe",
            b"18446744073709551616",
        ];
        let mut bytes = input.to_vec();
        let at = self.below(bytes.len());
        match self.below(6) {
            0 if !bytes.is_empty() => bytes[at] = self.next() as u8,
            1 => {
                let end = (at + self.below(40)).min(bytes.len());
                bytes.drain(at..end);
            }
            2 => {
                let end = (at + self.below(40)).min(bytes.len());
                let run = bytes[at..end].to_vec();
                for _ in 0..self.below(20) {
                    bytes.splice(at..at, run.iter().copied());
                }
            }
            3 => bytes.truncate(at),
            4 => {
                let piece = PIECES[self.below(PIECES.len())];
                bytes.splice(at..at, piece.iter().copied());
            }
            _ => {
                let piece = PIECES[self.below(PIECES.len())];
                let end = (at + piece.len()).min(bytes.len());
                bytes.splice(at..end, piece.iter().copied());
            }
        }
        bytes
    }
}

/// The files the EMBED goldens name, as loaded files.
fn golden_pool() -> Pool {
    Pool(vec![
        loaded("/root/single-frame.dcm", "sop:1.2.3.1", size(160, 240, 1)),
        loaded("/root/multiframe.dcm", "sop:1.2.3.2", size(160, 240, 3)),
        loaded("/root/comma, name.dcm", "sop:1.2.3.3", size(16, 16, 1)),
        loaded("/root/identical-copy.dcm", "sop:1.2.3.3", size(16, 16, 1)),
        loaded("nested/relative.dcm", "sop:1.2.3.5", size(4, 4, 3)),
        loaded("/link/single-frame.dcm", "sop:1.2.3.1", size(160, 240, 1)),
    ])
}

fn golden_csvs() -> Vec<(String, String)> {
    embed_golden_inputs()
        .into_iter()
        .map(|(name, text)| {
            (
                name,
                text.replace("{{ROOT}}", "/root")
                    .replace("{{LINK}}", "/link"),
            )
        })
        .collect()
}

/// The committed goldens are read as they are released to be read: every
/// one whose name does not say it is refused imports, and every other one
/// fails.
#[test]
fn the_golden_inputs_are_read_or_refused_as_their_names_say() {
    let pool = golden_pool();
    for (name, csv) in golden_csvs() {
        let result = read_rows(&mut csv.as_bytes(), &pool, &|| true);
        let refused = name.starts_with("rejected-") || name.starts_with("startup-");
        assert_eq!(result.is_err(), refused, "{name}: {result:?}");
    }
}

/// Every golden input, mutated a few thousand ways: the import returns,
/// and when it succeeds it kept its own promises.
#[test]
fn a_mutated_embed_csv_is_read_or_refused_and_never_more() {
    let pool = golden_pool();
    let session = Session::new(None);
    let mut mutations = Mutations(0x5eed_adb1_2026_1010);
    for (name, csv) in golden_csvs() {
        for round in 0..400 {
            let mut bytes = csv.clone().into_bytes();
            for _ in 0..=mutations.below(3) {
                bytes = mutations.mutate(&bytes);
            }
            let Ok(read) = read_rows(&mut bytes.as_slice(), &pool, &|| true) else {
                continue;
            };
            let mut slots = std::collections::HashSet::new();
            for file in &read.files {
                assert!(slots.insert(file.slot), "{name} #{round}: a file twice");
                let rois = &file.rois;
                assert!(
                    rois.roi_frames.is_empty() || rois.roi_frames.len() == rois.roi_coords.len(),
                    "{name} #{round}: frame lists"
                );
                let frames = pool.0[file.slot].size.frames;
                assert!(
                    rois.roi_frames
                        .iter()
                        .flatten()
                        .all(|frame| *frame < frames),
                    "{name} #{round}: a frame the file does not have"
                );
            }
            let handed: usize = read
                .files
                .iter()
                .map(|file| file.rois.roi_coords.len())
                .sum();
            assert_eq!(
                read.report.counts.annotations_out, handed as u64,
                "{name} #{round}"
            );
            // The adapter over the same bytes agrees on what it read.
            let imported = EmbedAdapter
                .import(&mut bytes.as_slice(), &session.context(&pool))
                .expect("the rows were readable");
            assert_eq!(
                imported.fragment.annotations.len(),
                handed,
                "{name} #{round}"
            );
            // And the registry never panics on its start.
            let _ = Registry::builtin().sniff(&bytes);
        }
    }
}

#[test]
fn a_mutated_native_document_is_read_or_refused_and_never_more() {
    let document = Document::from_json_str(&fixture_document_text()).expect("fixture");
    let compact = serde_json::to_string(&document).expect("text");
    let mut lines = Vec::new();
    dcmview_adapters::ExportAdapter::export(
        &dcmview_adapters::NativeJsonLines,
        &document,
        &dcmview_adapters::ExportContext {
            paths: &std::collections::BTreeMap::new(),
            max_bytes: u64::MAX,
            limits: Limits::DEFAULT,
        },
        &mut lines,
    )
    .expect("lines");
    let pool = Pool(vec![
        loaded(
            "cohort_a/patient_0001/mg/lcc.dcm",
            "sop:1.2.826.0.1.3680043.8.498.1",
            size(160, 240, 1),
        ),
        loaded(
            "cohort_a/patient_0001/mg/dbt.dcm",
            "sop:1.2.826.0.1.3680043.8.498.2",
            size(160, 240, 3),
        ),
    ]);
    let session = Session::new(document.schema.clone());
    let mut mutations = Mutations(0x0dd5_eed5_2026_1010);
    let mut read = 0;
    for (form, text) in [("document", compact.into_bytes()), ("lines", lines)] {
        for round in 0..1_500 {
            let mut bytes = text.clone();
            for _ in 0..=mutations.below(2) {
                bytes = mutations.mutate(&bytes);
            }
            let Ok(imported) = NativeJson.import(&mut bytes.as_slice(), &session.context(&pool))
            else {
                continue;
            };
            read += 1;
            let known = [
                key("sop:1.2.826.0.1.3680043.8.498.1"),
                key("sop:1.2.826.0.1.3680043.8.498.2"),
            ];
            for record in &imported.fragment.annotations {
                assert!(
                    known.contains(&record.file),
                    "{form} #{round}: a record on a file that was not matched"
                );
            }
            assert_eq!(
                imported.report.counts.annotations_out,
                imported.fragment.annotations.len() as u64,
                "{form} #{round}"
            );
            let _ = Registry::builtin().sniff(&bytes);
        }
    }
    assert!(read > 50, "the mutations left too little readable: {read}");
}

/// What a file says never comes back out in a report or in an error:
/// every free text of the input here holds a word that must not appear.
#[test]
fn a_report_and_an_error_hold_no_text_of_the_input() {
    const SECRET: &str = "DOE^JANE";
    let pool = Pool(vec![
        loaded("/data/DOE^JANE/a.dcm", "sop:1.2.3.1", size(160, 240, 1)),
        loaded("/data/DOE^JANE/m.dcm", "sop:1.2.3.2", size(160, 240, 3)),
    ]);
    let header = "anon_dicom_path,num_ROI,ROI_coords,ROI_frames,patient\n";
    let good = "/data/DOE^JANE/a.dcm,2,\"[[-1,2.5,300,400],[5,5,5,5]]\",[],DOE^JANE\n";
    let bad_rows = [
        "/data/DOE^JANE/m.dcm,DOE^JANE,\"[[1,2,3,4]]\",[]\n",
        "/data/DOE^JANE/m.dcm,1,\"[[1,2,3,\"\"DOE^JANE\"\"]]\",[]\n",
        "/data/DOE^JANE/m.dcm,1,\"[[1,2,3,4]]\",\"[[\"\"DOE^JANE\"\"]]\"\n",
        "/data/DOE^JANE/m.dcm,1,\"[[1,2,3,4]]\",[[7]]\n",
        "/data/DOE^JANE/a.dcm,1,\"[[1,2,3,4]]\",[]\n",
        "/data/DOE^JANE/m.dcm,1,DOE^JANE,[]\n",
        "/data/DOE^JANE/m.dcm,1,\"[[1,2,3,4]]\"\n",
    ];
    let read = read_rows(
        &mut format!("{header}{good}/nowhere/DOE^JANE.dcm,x,x,x,x\n").as_bytes(),
        &pool,
        &|| true,
    )
    .expect("read");
    assert!(read.report.warnings.len() >= 4);
    assert!(!report_json(&read.report).to_string().contains(SECRET));
    for row in bad_rows {
        let error = read_rows(
            &mut format!("{header}{good}{row}").as_bytes(),
            &pool,
            &|| true,
        )
        .expect_err(row);
        assert!(!error.to_string().contains(SECRET), "{error}");
        assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
    }

    // A native document with the word in every text a reader could quote.
    let session = Session::new(None);
    let pool = Pool(vec![loaded("a.dcm", "sop:1.2.3.1", size(160, 240, 1))]);
    let document = |class: &str, layer: &str, x1: &str| {
        format!(
            r#"{{"format":"dcmview.annotations","version":"1.0","producer":"DOE^JANE",
"files":[{{"key":"sop:1.2.3.1","kind":"dicom","sop_instance_uid":"1.2.3.1","patient_id":"DOE^JANE",
"path":"DOE^JANE/a.dcm","size_bytes":5,"rows":160,"columns":240,"frames":1,"format":"dicom"}},
{{"key":"sop:9.9.9","kind":"dicom","sop_instance_uid":"9.9.9","patient_id":"DOE^JANE",
"path":"DOE^JANE/b.dcm","size_bytes":5,"rows":160,"columns":240,"frames":1,"format":"dicom"}}],
"layers":[{{"id":"second","name":"DOE^JANE","kind":"user"}}],
"annotations":[{{"id":"0199c0de-0000-7000-8000-000000000001","file":"sop:1.2.3.1","frames":"all",
"layer":"{layer}","class":"{class}","geometry":{{"type":"rect","x0":1,"y0":2,"x1":{x1},"y1":900}},
"attributes":{{}},"extensions":{{"note":"DOE^JANE"}},"comment":"DOE^JANE",
"rev":1,"created_by":"user:DOE^JANE","created_at":"2026-10-10T08:00:00.000Z",
"modified_by":"user:DOE^JANE","modified_at":"2026-10-10T08:00:00.000Z"}}]}}"#
        )
    };
    let imported = NativeJson
        .import(
            &mut document("roi", "second", "3").as_bytes(),
            &session.context(&pool),
        )
        .expect("import");
    let report = report_json(&imported.report).to_string();
    assert!(report.contains("out_of_range") && report.contains("file_unmatched"));
    assert!(!report.contains(SECRET), "{report}");
    for text in [
        document("DOE^JANE", "second", "3"),
        document("roi", "DOE^JANE", "3"),
        document("roi", "second", "\"DOE^JANE\""),
        document("roi", "second", "3").replace("\"dcmview.annotations\"", "\"DOE^JANE\""),
        document("roi", "second", "3").replace("\"1.0\"", "\"DOE^JANE\""),
    ] {
        let error = NativeJson
            .import(&mut text.as_bytes(), &session.context(&pool))
            .expect_err("refused");
        assert!(!error.to_string().contains(SECRET), "{error}");
        assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
    }
}
