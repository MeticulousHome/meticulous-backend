//! The history API's search on a real machine's history: parity of the
//! transcoding path (`search_history_json`) with the parse+encode path
//! (`search_history` then tornado's encoding), and how long each takes for
//! the 20 and 100 newest shots.
//!
//!     search_bench <history dir> [--iterations N] [--full-check] [--bodies DIR]
//!
//! `<history dir>` holds `history.sqlite` and `shots/` (a machine's
//! `/meticulous-user/history`, or a copy). Nothing in it is written: the
//! database is snapshotted into a temporary directory (`VACUUM INTO`, which
//! is consistent also while the backend runs) and the shot files are read in
//! place. `--full-check` also compares both paths on every shot file and on
//! the listing of all shots (memory hungry on a big history); `--bodies`
//! writes the new GET /history bodies there as `rust_<n>.json`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use history::files::{decompress_file, read_patched, read_shot_json, read_shot_members};
use history::{HistoryDb, HistoryPaths, LocalZone, SearchParams};
use pyfmt::{DumpOpts, JsonStyle, Members};
use serde_json::Value;

const USAGE: &str =
    "usage: search_bench <history dir> [--iterations N] [--full-check] [--bodies DIR]";

fn shot_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            shot_files(&path, found);
        } else if path.to_string_lossy().ends_with(".zst") {
            found.push(path);
        }
    }
}

fn encode(value: &Value, style: JsonStyle) -> String {
    match style {
        JsonStyle::TORNADO => pyfmt::tornado_json_encode(value),
        JsonStyle::UNICODE => pyfmt::dumps(value, &DumpOpts::unicode()),
        _ => pyfmt::dumps_default(value),
    }
}

/// The first place two texts differ, with some context.
fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let context = |s: &str| {
        let bytes = &s.as_bytes()[at.saturating_sub(80)..(at + 80).min(s.len())];
        String::from_utf8_lossy(bytes).into_owned()
    };
    format!(
        "at byte {at}:\n  old: {}\n  new: {}",
        context(a),
        context(b)
    )
}

/// Every shot file, both readers, every style.
fn check_files(shots: &Path) {
    let mut files = Vec::new();
    shot_files(shots, &mut files);
    let (mut same, mut different) = (0, 0);
    for path in &files {
        let old = read_shot_json(path).ok();
        for style in [JsonStyle::DUMPS, JsonStyle::UNICODE, JsonStyle::TORNADO] {
            let old = old.as_ref().map(|value| {
                [
                    encode(value.get("data").unwrap_or(&Value::Null), style),
                    encode(
                        value.get("push_to_brew_time").unwrap_or(&Value::Null),
                        style,
                    ),
                ]
            });
            let new = match read_shot_members(path, style) {
                Ok(Members::Object([data, push])) => Some([
                    data.unwrap_or_else(|| "null".into()),
                    push.unwrap_or_else(|| "null".into()),
                ]),
                _ => None,
            };
            if old == new {
                same += 1;
                continue;
            }
            different += 1;
            let describe = |texts: &Option<[String; 2]>| match texts {
                Some([data, _]) => format!("{} bytes of data", data.len()),
                None => "unreadable".to_string(),
            };
            println!(
                "DIFFERENT {} ({style:?}): old {}, new {}",
                path.display(),
                describe(&old),
                describe(&new)
            );
            if let (Some(old), Some(new)) = (&old, &new) {
                println!("  {}", first_difference(&old[0], &new[0]));
            }
        }
    }
    println!(
        "files: {} shot files x 3 styles: {same} identical, {different} different",
        files.len()
    );
}

fn search(max_results: i64) -> SearchParams {
    SearchParams {
        max_results,
        ..SearchParams::default()
    }
}

fn old_body(db: &HistoryDb, params: &SearchParams) -> String {
    let rows = db.search_history(params).unwrap();
    pyfmt::tornado_json_encode(&pyfmt::object([("history", Value::Array(rows))]))
}

fn new_body(db: &HistoryDb, params: &SearchParams) -> String {
    let mut body = String::from("{\"history\": ");
    db.search_history_json(params, JsonStyle::TORNADO, &mut body)
        .unwrap();
    body.push('}');
    body
}

fn check_responses(db: &HistoryDb, sizes: &[i64], write_to: Option<&Path>) {
    for &max_results in sizes {
        let params = search(max_results);
        let (old, new) = (old_body(db, &params), new_body(db, &params));
        if let Some(dir) = write_to {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("rust_{max_results}.json")), &new).unwrap();
        }
        if old == new {
            println!(
                "response max_results={max_results}: identical ({} bytes)",
                new.len()
            );
        } else {
            println!(
                "response max_results={max_results}: DIFFERENT ({} vs {} bytes), {}",
                old.len(),
                new.len(),
                first_difference(&old, &new)
            );
        }
    }
}

fn time(iterations: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    let mut runs: Vec<Duration> = (0..iterations.max(1))
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    runs.sort();
    (runs[0], runs[runs.len() / 2])
}

fn ms(d: Duration) -> String {
    format!("{:7.2} ms", d.as_secs_f64() * 1000.0)
}

fn bench(db: &HistoryDb, shots: &Path, iterations: usize) {
    for max_results in [20, 100] {
        let params = search(max_results);
        let listing = SearchParams {
            dump_data: false,
            ..search(max_results)
        };
        let files: Vec<PathBuf> = db
            .search_history(&listing)
            .unwrap()
            .iter()
            .map(|row| shots.join(row["file"].as_str().unwrap()))
            .collect();
        let (search_min, search_med) = time(iterations, || {
            std::hint::black_box(db.search_history(&params).unwrap());
        });
        let wrapped =
            pyfmt::object([("history", Value::Array(db.search_history(&params).unwrap()))]);
        let (encode_min, encode_med) = time(iterations, || {
            std::hint::black_box(pyfmt::tornado_json_encode(&wrapped));
        });
        drop(wrapped);
        let mut body_len = 0;
        let (new_min, new_med) = time(iterations, || {
            body_len = std::hint::black_box(new_body(db, &params)).len();
        });
        // Where the new path spends it: decompressing, the non-finite patch
        // check, transcoding.
        let (decompress_min, _) = time(iterations, || {
            for path in &files {
                std::hint::black_box(decompress_file(path, None).unwrap());
            }
        });
        let (read_min, _) = time(iterations, || {
            for path in &files {
                std::hint::black_box(read_patched(path).unwrap());
            }
        });
        let texts: Vec<String> = files
            .iter()
            .map(|p| String::from_utf8(read_patched(p).unwrap()).unwrap())
            .collect();
        let (transcode_min, _) = time(iterations, || {
            for text in &texts {
                std::hint::black_box(
                    pyfmt::transcode_members(
                        text,
                        ["data", "push_to_brew_time"],
                        JsonStyle::TORNADO,
                    )
                    .unwrap(),
                );
            }
        });
        let input: usize = texts.iter().map(String::len).sum();
        println!(
            "{} shots ({:.1} MB of shot JSON, {:.2} MB response; min / median of {iterations}):",
            files.len(),
            input as f64 / 1e6,
            body_len as f64 / 1e6
        );
        println!(
            "  before: search_history {} / {}  + tornado encode {} / {}  = {}",
            ms(search_min),
            ms(search_med),
            ms(encode_min),
            ms(encode_med),
            ms(search_min + encode_min)
        );
        println!(
            "  after:  search_history_json {} / {}  (decompress {}, with patch check {}, transcode {})",
            ms(new_min),
            ms(new_med),
            ms(decompress_min),
            ms(read_min),
            ms(transcode_min)
        );
    }
}

fn main() {
    let (mut source, mut iterations, mut full, mut bodies) = (None, 15, false, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--full-check" => full = true,
            "--bodies" => bodies = Some(PathBuf::from(args.next().expect(USAGE))),
            "--iterations" => {
                iterations = args.next().and_then(|n| n.parse().ok()).expect(USAGE);
            }
            _ if source.is_none() && !arg.starts_with('-') => source = Some(PathBuf::from(arg)),
            _ => panic!("{USAGE}"),
        }
    }
    let source = source.expect(USAGE);
    let work = std::env::temp_dir().join(format!("search-bench-{}", std::process::id()));
    let history = work.join("history");
    std::fs::create_dir_all(&history).unwrap();
    // A live database (the backend has it open) has its -wal and -shm: read
    // through them. A copy without them is read as immutable, so that SQLite
    // creates nothing next to it.
    let db_file = source.join("history.sqlite");
    let live = PathBuf::from(format!("{}-wal", db_file.display())).exists();
    let uri = format!(
        "file:{}?mode=ro{}",
        db_file.display(),
        if live { "" } else { "&immutable=1" }
    );
    let original = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("history.sqlite opens");
    original
        .execute(
            "VACUUM INTO ?1",
            [history.join("history.sqlite").to_string_lossy()],
        )
        .expect("database snapshot");
    drop(original);
    std::os::unix::fs::symlink(source.join("shots"), history.join("shots")).unwrap();
    let paths = HistoryPaths::new(history.clone(), work.join("dbmigrations"));
    let db = HistoryDb::open(paths, LocalZone::System).expect("database opens");
    db.migrate().expect("database migrates");
    let shots = history.join("shots");
    if full {
        check_files(&shots);
    }
    let sizes: &[i64] = if full { &[20, 100, 0] } else { &[20, 100] };
    check_responses(&db, sizes, bodies.as_deref());
    bench(&db, &shots, iterations);
    drop(db);
    // Removes the link to the shots, not the shots.
    std::fs::remove_dir_all(&work).unwrap();
}
