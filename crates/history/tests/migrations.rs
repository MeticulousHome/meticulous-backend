//! The migration engine against the databases alembic builds.
//!
//! Every scenario of `tests/data/migrations.json` (made by
//! `generate_migrations.py` with the Python updater and alembic) is replayed
//! with the Rust engine; the resulting `sqlite_master`, rows, alembic revision
//! and failure must match. Also the cases of `tests/test_db_migrations.py`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Env, HEAD, REVISIONS, assert_same_database, dump, insert_sample, load_json};
use history::migrate::{self, DB_VERSION_REQUIRED};
use serde_json::Value;

fn reference(name: &str) -> Value {
    let all = load_json("migrations.json");
    all["scenarios"]
        .get(name)
        .unwrap_or_else(|| panic!("scenario {name} in migrations.json"))
        .clone()
}

fn check(name: &str, env: &Env, error: Option<String>) {
    let expected = reference(name);
    assert_eq!(
        error.is_some(),
        !expected["error"].is_null(),
        "[{name}] Rust error {error:?}, Python error {}",
        expected["error"]
    );
    assert_same_database(name, &expected, &dump(&env.db_file()));
}

/// The pre-alembic tables as `metadata.create_all()` rendered them (c7162db^).
const PRE_ALEMBIC: [&str; 2] = [
    "\nCREATE TABLE profile (\n\t\"key\" INTEGER NOT NULL, \n\tid VARCHAR NOT NULL, \n\tauthor TEXT, \n\tauthor_id TEXT, \n\tdisplay JSON, \n\tfinal_weight INTEGER, \n\tlast_changed FLOAT, \n\tname TEXT, \n\ttemperature INTEGER, \n\tstages JSON, \n\tvariables JSON, \n\tprevious_authors JSON, \n\tPRIMARY KEY (\"key\")\n)\n\n",
    "\nCREATE TABLE history (\n\tid INTEGER NOT NULL, \n\tuuid TEXT, \n\tfile TEXT NOT NULL, \n\ttime DATETIME NOT NULL, \n\tprofile_name TEXT NOT NULL, \n\tprofile_id VARCHAR NOT NULL, \n\tprofile_key INTEGER NOT NULL, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(profile_key) REFERENCES profile (\"key\")\n)\n\n",
];
const PRE_ALEMBIC_RATINGS: &str = "\nCREATE TABLE shot_ratings (\n\tid INTEGER NOT NULL, \n\thistory_id INTEGER NOT NULL, \n\trating VARCHAR DEFAULT 'unrated' NOT NULL, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(history_id) REFERENCES history (id) ON DELETE CASCADE\n)\n\n";

fn pre_alembic(env: &Env, ratings_table: bool) {
    let conn = env.connect();
    for ddl in PRE_ALEMBIC {
        conn.execute_batch(ddl).unwrap();
    }
    if ratings_table {
        conn.execute_batch(PRE_ALEMBIC_RATINGS).unwrap();
    }
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts USING fts5(profile_key, profile_id, name)",
    )
    .unwrap();
    conn.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts USING fts5(profile_key, profile_id, profile_name, stage_key, stage_name)").unwrap();
    drop(conn);
    insert_sample(env, "ebb6a77afd0e", true);
    if ratings_table {
        env.connect()
            .execute_batch(
                "INSERT INTO shot_ratings (id, history_id, rating) VALUES (1, 1, 'good')",
            )
            .unwrap();
    }
}

/// 0bdd1c635e7a as the Feb-May 2025 images created it: no constraint names.
fn legacy_annotations(env: &Env, ratings: bool) {
    env.open().migrate_to("ebb6a77afd0e").unwrap();
    let conn = env.connect();
    conn.execute_batch("\nCREATE TABLE shot_annotation (\n\tid INTEGER NOT NULL, \n\thistory_id INTEGER NOT NULL, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(history_id) REFERENCES history (id), \n\tUNIQUE (history_id)\n)\n\n").unwrap();
    conn.execute_batch("\nCREATE TABLE shot_rating (\n\tid INTEGER NOT NULL, \n\tannotation_id INTEGER NOT NULL, \n\tbasic TEXT, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(annotation_id) REFERENCES shot_annotation (id), \n\tUNIQUE (annotation_id)\n)\n\n").unwrap();
    conn.execute_batch("UPDATE alembic_version SET version_num = '0bdd1c635e7a'")
        .unwrap();
    drop(conn);
    insert_sample(env, "0bdd1c635e7a", ratings);
}

/// test_a_new_database_is_migrated_to_the_schema_of_the_models: a new machine
/// boots (init, then the updater) into the schema the Python boot produces.
#[test]
fn boot_of_a_new_machine_matches_python() {
    let env = Env::new();
    let error = env.boot();
    check("boot_new", &env, error);
    assert_eq!(
        env.open().current_revision().unwrap().as_deref(),
        Some(DB_VERSION_REQUIRED)
    );
}

/// test_boot_backs_up_every_revision_script: the backup directory holds every
/// revision script, under the names the Python image uses.
#[test]
fn boot_backs_up_every_revision_script() {
    let env = Env::new();
    assert_eq!(env.boot(), None);
    let mut backed_up: Vec<String> = std::fs::read_dir(env.paths().migration_backup_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    backed_up.sort();
    let expected: Vec<String> = reference("boot_new")["backup"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(backed_up, expected);
}

/// The backed-up scripts are byte-identical to `alembic/versions/` while the
/// Python sources are in the tree, so an older image can run their downgrade.
#[test]
fn backed_up_scripts_are_the_python_revision_scripts() {
    let versions = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alembic/versions");
    if !versions.is_dir() {
        return;
    }
    let env = Env::new();
    assert_eq!(env.boot(), None);
    for entry in std::fs::read_dir(&versions).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        let name = path.file_name().unwrap();
        let backed_up = std::fs::read(env.paths().migration_backup_dir.join(name))
            .unwrap_or_else(|_| panic!("{name:?} was not backed up"));
        assert_eq!(backed_up, std::fs::read(&path).unwrap(), "{name:?}");
    }
}

/// A script already in the backup directory is left alone (backup_new_scripts
/// copies only missing files).
#[test]
fn existing_backups_are_kept() {
    let env = Env::new();
    let backup = env.paths().migration_backup_dir;
    std::fs::create_dir_all(&backup).unwrap();
    let name = "2025_02_05_1816-ebb6a77afd0e_initial_migration.py";
    std::fs::write(backup.join(name), "# kept\n").unwrap();
    std::fs::write(
        backup.join("9999_newer_revision.py"),
        "# from a newer image\n",
    )
    .unwrap();
    assert_eq!(env.boot(), None);
    assert_eq!(
        std::fs::read_to_string(backup.join(name)).unwrap(),
        "# kept\n"
    );
    assert!(backup.join("9999_newer_revision.py").exists());
}

/// test_the_single_head_is_the_revision_the_backend_requires.
#[test]
fn the_head_is_the_required_revision() {
    assert_eq!(
        migrate::revision_ids().last().copied(),
        Some(DB_VERSION_REQUIRED)
    );
    assert_eq!(migrate::revision_ids(), REVISIONS.to_vec());
    for (index, revision) in migrate::REVISIONS.iter().enumerate() {
        let expected_down = index.checked_sub(1).map(|i| REVISIONS[i]);
        assert_eq!(revision.down, expected_down, "{}", revision.id);
    }
}

/// The database at each revision, as `alembic upgrade <rev>` builds it after
/// `ShotDataBase.init()`.
#[test]
fn each_revision_matches_alembic() {
    for revision in REVISIONS {
        let env = Env::new();
        env.open().migrate_to(revision).unwrap();
        check(&format!("fresh/{revision}"), &env, None);
    }
}

/// test_each_revision_downgrades_and_upgrades_again, schema by schema: from
/// the head down to each revision.
#[test]
fn each_downgrade_matches_alembic() {
    for revision in &REVISIONS[..6] {
        let env = Env::new();
        let db = env.open();
        db.migrate_to(HEAD).unwrap();
        let error = db.migrate_to(revision).err().map(|e| e.to_string());
        drop(db);
        check(&format!("downgrade/{revision}"), &env, error);
    }
}

/// test_each_revision_downgrades_and_upgrades_again: every step down, then
/// back up to the head schema.
#[test]
fn each_revision_downgrades_and_upgrades_again() {
    let env = Env::new();
    let db = env.open();
    db.migrate_to(HEAD).unwrap();
    for revision in REVISIONS[..6].iter().rev() {
        db.migrate_to(revision).unwrap();
        assert_eq!(db.current_revision().unwrap().as_deref(), Some(*revision));
    }
    db.migrate_to(HEAD).unwrap();
    assert_eq!(db.current_revision().unwrap().as_deref(), Some(HEAD));
    drop(db);
    let fresh = Env::new();
    fresh.open().migrate_to(HEAD).unwrap();
    // Same tables, constraints and indexes as a database built upwards.
    let names = |d: &Value| -> Vec<(String, String)> {
        d["master"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["name"].as_str().unwrap().to_string(),
                    m["sql"]
                        .as_str()
                        .map(|s| {
                            common::normalise_sql(s)
                                .replace("TABLE \"", "TABLE ")
                                .replace("\" (", " (")
                        })
                        .unwrap_or_default(),
                )
            })
            .collect()
    };
    assert_eq!(names(&dump(&env.db_file())), names(&dump(&fresh.db_file())));
}

/// test_history_from_the_first_release_survives_the_upgrade, and the same
/// for every older revision with data in every table it has.
#[test]
fn upgrades_with_data_match_alembic() {
    for revision in &REVISIONS[..6] {
        let variants: &[bool] = if *revision == "0bdd1c635e7a" {
            &[true, false]
        } else {
            &[true]
        };
        for ratings in variants {
            let name = format!(
                "data_upgrade/{revision}{}",
                if *ratings { "" } else { "/no_ratings" }
            );
            let env = Env::new();
            env.open().migrate_to(revision).unwrap();
            insert_sample(&env, revision, *ratings);
            let error = env.boot();
            check(&name, &env, error);
        }
    }
}

/// test_shot_ratings_from_before_history_uuid_survive_the_upgrade (a strict
/// xfail in Python): the upgrade from 0bdd1c635e7a fails on a rated shot, the
/// database stays at 0bdd1c635e7a and keeps alembic's leftover temp table.
#[test]
fn rated_shots_before_history_uuid_fail_the_upgrade_like_python() {
    let env = Env::new();
    env.open().migrate_to("0bdd1c635e7a").unwrap();
    insert_sample(&env, "0bdd1c635e7a", true);
    let error = env.boot();
    assert!(
        error
            .as_deref()
            .unwrap_or_default()
            .contains("NOT NULL constraint failed")
    );
    assert_eq!(
        env.open().current_revision().unwrap().as_deref(),
        Some("0bdd1c635e7a")
    );
    let tables: Vec<String> = dump(&env.db_file())["master"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect();
    assert!(tables.contains(&"_alembic_tmp_shot_annotation".to_string()));
}

/// A machine at the head with data rolls back to each revision (the
/// downgrades an older image runs), then updates again.
#[test]
fn downgrades_with_data_match_alembic() {
    for revision in &REVISIONS[..6] {
        let env = Env::new();
        env.open().migrate_to(HEAD).unwrap();
        insert_sample(&env, HEAD, true);
        let error = env.open().migrate_to(revision).err().map(|e| e.to_string());
        check(&format!("data_downgrade/{revision}"), &env, error);
        let error = env.boot();
        check(&format!("data_roundtrip/{revision}"), &env, error);
    }
}

/// A database created before alembic existed (the initial revision's
/// `table_exists` checks).
#[test]
fn pre_alembic_databases_match_alembic() {
    for ratings_table in [false, true] {
        let name = format!(
            "pre_alembic{}",
            if ratings_table { "/shot_ratings" } else { "" }
        );
        let env = Env::new();
        pre_alembic(&env, ratings_table);
        let error = env.boot();
        check(&name, &env, error);
    }
    for revision in ["1a598cd3ace3", "ebb6a77afd0e"] {
        let env = Env::new();
        pre_alembic(&env, false);
        assert_eq!(env.boot(), None);
        let error = env.open().migrate_to(revision).err().map(|e| e.to_string());
        check(&format!("pre_alembic_downgrade/{revision}"), &env, error);
    }
}

/// Machines from Feb-May 2025: annotation tables without constraint names.
#[test]
fn legacy_annotation_tables_match_alembic() {
    for ratings in [false, true] {
        let name = format!(
            "legacy_0bdd1c635e7a{}",
            if ratings { "/ratings" } else { "" }
        );
        let env = Env::new();
        legacy_annotations(&env, ratings);
        let error = env.boot();
        check(&name, &env, error);
    }
}

/// test_a_rollback_downgrades_with_the_script_the_newer_image_backed_up, from
/// the Rust side: a database at a revision Rust does not know is left alone
/// and reported, like the Python updater without the script.
#[test]
fn an_unknown_revision_is_reported_and_left_alone() {
    let env = Env::new();
    assert_eq!(env.boot(), None);
    env.connect()
        .execute_batch("UPDATE alembic_version SET version_num = 'ffffffffffff'")
        .unwrap();
    let before = dump(&env.db_file());
    let error = env.boot().expect("an unknown revision fails");
    assert!(error.contains("Cannot retrieve missing scripts"), "{error}");
    assert_eq!(dump(&env.db_file()), before);
}

/// An up-to-date database is not touched.
#[test]
fn an_up_to_date_database_is_left_alone() {
    let env = Env::new();
    assert_eq!(env.boot(), None);
    insert_sample(&env, HEAD, true);
    let before = dump(&env.db_file());
    let db = env.open();
    assert_eq!(
        db.migrate().unwrap(),
        history::MigrationOutcome::AlreadyCurrent
    );
    drop(db);
    assert_eq!(dump(&env.db_file()), before);
}
