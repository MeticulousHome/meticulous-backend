//! The alembic revisions in `alembic/versions/`, operation for operation.
//!
//! Each `CREATE TABLE` is the text SQLAlchemy renders for the revision's
//! `op.create_table(...)` under `database_models.metadata`'s naming
//! convention (alembic applies the target metadata's convention to op-level
//! constraints). SQLite stores that text in `sqlite_master`, so it is part of
//! the on-disk contract.

use super::batch::{BatchOp, NewColumn, batch_alter_table};
use super::sql::{StepRunner, quote};
use crate::error::Result;

/// The Python revision scripts, byte for byte. They are written to the
/// backup directory so an older Python image can downgrade what Rust applied.
pub struct Script {
    pub file_name: &'static str,
    pub contents: &'static str,
}

pub struct Revision {
    pub id: &'static str,
    pub down: Option<&'static str>,
    pub script: Script,
    pub upgrade: fn(&mut Ops<'_, '_>) -> Result<()>,
    pub downgrade: fn(&mut Ops<'_, '_>) -> Result<()>,
}

/// The subset of `alembic.op` the revisions call.
pub struct Ops<'r, 'c> {
    pub run: &'r mut StepRunner<'c>,
}

impl Ops<'_, '_> {
    /// `inspect(op.get_bind()).get_table_names()` contains `name`.
    fn table_exists(&self, name: &str) -> Result<bool> {
        let mut statement = self.run.conn().prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite~_%' ESCAPE '~' ORDER BY name",
        )?;
        let names = statement.query_map([], |row| row.get::<_, String>(0))?;
        for table in names {
            if table? == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn execute(&mut self, sql: &str) -> Result<()> {
        self.run.exec(sql)
    }

    fn drop_table(&mut self, name: &str) -> Result<()> {
        self.run.exec(&format!("\nDROP TABLE {}", quote(name)))
    }

    fn create_index(&mut self, name: &str, table: &str, columns: &[&str]) -> Result<()> {
        let columns: Vec<String> = columns.iter().map(|c| quote(c)).collect();
        self.run.exec(&format!(
            "CREATE INDEX {} ON {} ({})",
            quote(name),
            quote(table),
            columns.join(", ")
        ))
    }

    fn drop_index(&mut self, name: &str) -> Result<()> {
        self.run.exec(&format!("\nDROP INDEX {}", quote(name)))
    }

    fn batch(&mut self, table: &str, ops: &[BatchOp]) -> Result<()> {
        batch_alter_table(self.run, table, ops)
    }
}

pub const PROFILE_DDL: &str = "\nCREATE TABLE profile (\n\t\"key\" INTEGER NOT NULL, \n\tid TEXT NOT NULL, \n\tauthor TEXT, \n\tauthor_id TEXT, \n\tdisplay JSON, \n\tfinal_weight INTEGER, \n\tlast_changed FLOAT, \n\tname TEXT, \n\ttemperature INTEGER, \n\tstages JSON, \n\tvariables JSON, \n\tprevious_authors JSON, \n\tCONSTRAINT pk_profile PRIMARY KEY (\"key\")\n)\n\n";

pub const HISTORY_DDL: &str = "\nCREATE TABLE history (\n\tid INTEGER NOT NULL, \n\tuuid TEXT, \n\tfile TEXT NOT NULL, \n\ttime DATETIME NOT NULL, \n\tprofile_name TEXT NOT NULL, \n\tprofile_id TEXT NOT NULL, \n\tprofile_key INTEGER NOT NULL, \n\tCONSTRAINT pk_history PRIMARY KEY (id), \n\tCONSTRAINT fk_history_profile_key_profile FOREIGN KEY(profile_key) REFERENCES profile (\"key\")\n)\n\n";

/// The FTS tables as the initial revision creates them (multi-line text).
pub const PROFILE_FTS_MIGRATION_DDL: &str = "\n        CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts\n        USING fts5(\n            profile_key,\n            profile_id,\n            name\n        )\n    ";

pub const STAGE_FTS_MIGRATION_DDL: &str = "\n        CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts\n        USING fts5(\n            profile_key,\n            profile_id,\n            profile_name,\n            stage_key,\n            stage_name\n        )\n    ";

/// The pragmas the initial revision runs (the same ones the connect hook runs).
pub const INITIAL_PRAGMAS: [&str; 5] = [
    "PRAGMA auto_vacuum=full",
    "PRAGMA journal_mode=WAL",
    "PRAGMA synchronous=EXTRA",
    "PRAGMA journal_size_limit = 1048576",
    "PRAGMA wal_checkpoint(TRUNCATE)",
];

pub const SHOT_ANNOTATION_DDL: &str = "\nCREATE TABLE shot_annotation (\n\tid INTEGER NOT NULL, \n\thistory_id INTEGER NOT NULL, \n\tCONSTRAINT pk_shot_annotation PRIMARY KEY (id), \n\tCONSTRAINT fk_shot_annotation_history_id_history FOREIGN KEY(history_id) REFERENCES history (id), \n\tCONSTRAINT uq_shot_annotation_history_id UNIQUE (history_id)\n)\n\n";

pub const SHOT_RATING_DDL: &str = "\nCREATE TABLE shot_rating (\n\tid INTEGER NOT NULL, \n\tannotation_id INTEGER NOT NULL, \n\tbasic TEXT, \n\tCONSTRAINT pk_shot_rating PRIMARY KEY (id), \n\tCONSTRAINT fk_shot_rating_annotation_id_shot_annotation FOREIGN KEY(annotation_id) REFERENCES shot_annotation (id), \n\tCONSTRAINT uq_shot_rating_annotation_id UNIQUE (annotation_id)\n)\n\n";

pub const BUG_REPORTS_DDL: &str = "\nCREATE TABLE bug_reports (\n\t\"localID\" TEXT NOT NULL, \n\t\"eventID\" TEXT, \n\t\"baseEventID\" TEXT, \n\t\"issueTime\" INTEGER NOT NULL, \n\t\"creationTime\" INTEGER NOT NULL, \n\t\"submissionTime\" INTEGER, \n\tdescription TEXT, \n\tmultimedia INTEGER, \n\t\"machineID\" TEXT, \n\t\"logFiles\" TEXT, \n\t\"machineInfo\" BOOLEAN, \n\t\"machineLogs\" BOOLEAN, \n\t\"machineStatus\" BOOLEAN, \n\tstatus TEXT NOT NULL, \n\t\"ticketNumber\" INTEGER, \n\tCONSTRAINT pk_bug_reports PRIMARY KEY (\"localID\")\n)\n\n";

pub const BREW_HISTORY_DDL: &str = "\nCREATE TABLE brew_history (\n\tid INTEGER NOT NULL, \n\tuuid TEXT NOT NULL, \n\tbrew_type TEXT NOT NULL, \n\tmode TEXT NOT NULL, \n\tfile TEXT NOT NULL, \n\ttime DATETIME NOT NULL, \n\tcompleted_time DATETIME NOT NULL, \n\tname TEXT NOT NULL, \n\tschema_version INTEGER NOT NULL, \n\tCONSTRAINT pk_brew_history PRIMARY KEY (id), \n\tCONSTRAINT uq_brew_history_file UNIQUE (file), \n\tCONSTRAINT uq_brew_history_uuid UNIQUE (uuid)\n)\n\n";

// ebb6a77afd0e initial_migration
fn initial_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    if !op.table_exists("profile")? {
        op.execute(PROFILE_DDL)?;
    }
    if !op.table_exists("history")? {
        op.execute(HISTORY_DDL)?;
    }
    op.execute(PROFILE_FTS_MIGRATION_DDL)?;
    op.execute(STAGE_FTS_MIGRATION_DDL)?;
    for pragma in INITIAL_PRAGMAS {
        op.execute(pragma)?;
    }
    Ok(())
}

fn initial_downgrade(_op: &mut Ops<'_, '_>) -> Result<()> {
    Ok(())
}

// 0bdd1c635e7a shot annotation
fn annotation_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    if !op.table_exists("shot_annotation")? {
        op.execute(SHOT_ANNOTATION_DDL)?;
    }
    if !op.table_exists("shot_rating")? {
        op.execute(SHOT_RATING_DDL)?;
    }
    Ok(())
}

fn annotation_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    if op.table_exists("shot_rating")? {
        op.drop_table("shot_rating")?;
    }
    if op.table_exists("shot_annotation")? {
        op.drop_table("shot_annotation")?;
    }
    Ok(())
}

// 1a598cd3ace3 add history_uuid to shot annotation
fn history_uuid_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch(
        "shot_annotation",
        &[
            BatchOp::CreateUnique {
                columns: vec!["history_id"],
            },
            BatchOp::CreateForeignKey {
                referent: "history",
                local: vec!["history_id"],
                remote: vec!["id"],
            },
        ],
    )?;
    op.batch(
        "shot_rating",
        &[BatchOp::CreateUnique {
            columns: vec!["annotation_id"],
        }],
    )?;
    // PARITY: history_uuid is NOT NULL without a value for existing rows, so a
    // database with a shot rating fails here (tests/test_db_migrations.py
    // xfail) and keeps the empty _alembic_tmp_shot_annotation that the first
    // batch created before pysqlite opened the step's transaction.
    op.batch(
        "shot_annotation",
        &[
            BatchOp::AddColumn(NewColumn {
                name: "history_uuid",
                type_sql: "TEXT",
                nullable: false,
            }),
            BatchOp::CreateUnique {
                columns: vec!["history_uuid"],
            },
            BatchOp::CreateForeignKey {
                referent: "history",
                local: vec!["history_uuid"],
                remote: vec!["uuid"],
            },
        ],
    )
}

fn history_uuid_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch(
        "shot_annotation",
        &[
            BatchOp::DropConstraint("fk_shot_annotation_history_id_history"),
            BatchOp::DropConstraint("uq_shot_annotation_history_id"),
            BatchOp::DropConstraint("fk_shot_annotation_history_uuid_history"),
            BatchOp::DropConstraint("uq_shot_annotation_history_uuid"),
            BatchOp::DropColumn("history_uuid"),
        ],
    )?;
    op.batch(
        "shot_rating",
        &[
            BatchOp::DropConstraint("fk_shot_rating_annotation_id_shot_annotation"),
            BatchOp::DropConstraint("uq_shot_rating_annotation_id"),
        ],
    )
}

// 470a6d3b0f44 debug_file column
fn debug_file_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch(
        "history",
        &[BatchOp::AddColumn(NewColumn {
            name: "debug_file",
            type_sql: "TEXT",
            nullable: true,
        })],
    )
}

fn debug_file_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch("history", &[BatchOp::DropColumn("debug_file")])
}

// 8f4e7b2c9d10 bug reports table
fn bug_reports_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.execute(BUG_REPORTS_DDL)?;
    op.create_index(
        "ix_bug_reports_creationTime",
        "bug_reports",
        &["creationTime"],
    )?;
    op.create_index("ix_bug_reports_status", "bug_reports", &["status"])
}

fn bug_reports_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.drop_index("ix_bug_reports_status")?;
    op.drop_index("ix_bug_reports_creationTime")?;
    op.drop_table("bug_reports")
}

// b71d84a4c2ef brew history table
fn brew_history_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.execute(BREW_HISTORY_DDL)?;
    op.create_index(
        "ix_brew_history_completed_time",
        "brew_history",
        &["completed_time"],
    )?;
    op.create_index("ix_brew_history_brew_type", "brew_history", &["brew_type"])
}

fn brew_history_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.drop_index("ix_brew_history_brew_type")?;
    op.drop_index("ix_brew_history_completed_time")?;
    op.drop_table("brew_history")
}

// c4d2e9f1a7b3 bug report contact columns
fn contact_upgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch(
        "bug_reports",
        &[
            BatchOp::AddColumn(NewColumn {
                name: "contactName",
                type_sql: "TEXT",
                nullable: true,
            }),
            BatchOp::AddColumn(NewColumn {
                name: "contactEmail",
                type_sql: "TEXT",
                nullable: true,
            }),
        ],
    )
}

fn contact_downgrade(op: &mut Ops<'_, '_>) -> Result<()> {
    op.batch(
        "bug_reports",
        &[
            BatchOp::DropColumn("contactEmail"),
            BatchOp::DropColumn("contactName"),
        ],
    )
}

/// The revision chain, oldest first.
pub const REVISIONS: [Revision; 7] = [
    Revision {
        id: "ebb6a77afd0e",
        down: None,
        script: Script {
            file_name: "2025_02_05_1816-ebb6a77afd0e_initial_migration.py",
            contents: include_str!(
                "../../alembic_scripts/2025_02_05_1816-ebb6a77afd0e_initial_migration.py.txt"
            ),
        },
        upgrade: initial_upgrade,
        downgrade: initial_downgrade,
    },
    Revision {
        id: "0bdd1c635e7a",
        down: Some("ebb6a77afd0e"),
        script: Script {
            file_name: "2025_02_22_0053-0bdd1c635e7a_shot_annotation.py",
            contents: include_str!(
                "../../alembic_scripts/2025_02_22_0053-0bdd1c635e7a_shot_annotation.py.txt"
            ),
        },
        upgrade: annotation_upgrade,
        downgrade: annotation_downgrade,
    },
    Revision {
        id: "1a598cd3ace3",
        down: Some("0bdd1c635e7a"),
        script: Script {
            file_name: "2025_05_05_1722-ed8465b1592c_add_history_uuid_to_annotations_table.py",
            contents: include_str!(
                "../../alembic_scripts/2025_05_05_1722-ed8465b1592c_add_history_uuid_to_annotations_table.py.txt"
            ),
        },
        upgrade: history_uuid_upgrade,
        downgrade: history_uuid_downgrade,
    },
    Revision {
        id: "470a6d3b0f44",
        down: Some("1a598cd3ace3"),
        script: Script {
            file_name: "2025_07_15_1307-470a6d3b0f44_create_debug_file_column_to_history_.py",
            contents: include_str!(
                "../../alembic_scripts/2025_07_15_1307-470a6d3b0f44_create_debug_file_column_to_history_.py.txt"
            ),
        },
        upgrade: debug_file_upgrade,
        downgrade: debug_file_downgrade,
    },
    Revision {
        id: "8f4e7b2c9d10",
        down: Some("470a6d3b0f44"),
        script: Script {
            file_name: "2026_05_18_1200-8f4e7b2c9d10_add_bug_reports_table.py",
            contents: include_str!(
                "../../alembic_scripts/2026_05_18_1200-8f4e7b2c9d10_add_bug_reports_table.py.txt"
            ),
        },
        upgrade: bug_reports_upgrade,
        downgrade: bug_reports_downgrade,
    },
    Revision {
        id: "b71d84a4c2ef",
        down: Some("8f4e7b2c9d10"),
        script: Script {
            file_name: "2026_08_16_1500-b71d84a4c2ef_add_brew_history_table.py",
            contents: include_str!(
                "../../alembic_scripts/2026_08_16_1500-b71d84a4c2ef_add_brew_history_table.py.txt"
            ),
        },
        upgrade: brew_history_upgrade,
        downgrade: brew_history_downgrade,
    },
    Revision {
        id: "c4d2e9f1a7b3",
        down: Some("b71d84a4c2ef"),
        script: Script {
            file_name: "2026_10_01_1200-c4d2e9f1a7b3_add_bug_report_contact_columns.py",
            contents: include_str!(
                "../../alembic_scripts/2026_10_01_1200-c4d2e9f1a7b3_add_bug_report_contact_columns.py.txt"
            ),
        },
        upgrade: contact_upgrade,
        downgrade: contact_downgrade,
    },
];
