import json
import logging
import os
import sqlite3
import uuid
from datetime import datetime

import coloredlogs
import pytz
import sqlalchemy
import sqlite_zstd
import zstandard as zstd
from flask import Flask, jsonify, render_template_string, request
from sqlalchemy import (JSON, Column, DateTime, Float, ForeignKey, Integer,
                        MetaData, String, Table, Text, create_engine, delete)
from sqlalchemy import event as sqlEvent
from sqlalchemy import func, insert, or_, select, text
from sqlalchemy.orm import sessionmaker


import traceback

coloredlogs.install(level="DEBUG")

# Initialize database connection
DATABASE_FILE = "example.db"
DATABASE_URL = "sqlite:///" + DATABASE_FILE
engine = create_engine(
    DATABASE_URL, echo=True, connect_args={"check_same_thread": False}
)


@sqlEvent.listens_for(engine, "connect")
def setupDatabase(dbapi_connection, _connection_record):
    dbapi_connection.execute("PRAGMA auto_vacuum=full;")
    dbapi_connection.execute("PRAGMA journal_mode=WAL;")
    dbapi_connection.execute("PRAGMA synchronous=EXTRA;")

    sqlite_zstd.load(dbapi_connection)


metadata = MetaData()
Session = sessionmaker(bind=engine)

# Define tables
profile_table = Table(
    "profile",
    metadata,
    Column("key", Integer, primary_key=True, autoincrement=True),
    Column("id", String, nullable=False),
    Column("author", Text),
    Column("author_id", Text),
    Column("display_image", Text),
    Column("final_weight", Integer),
    Column("last_changed", Float),
    Column("name", Text),
    Column("temperature", Integer),
    Column("stages", JSON),
    Column("variables", JSON),
    Column("previous_authors", JSON),
)

history_table = Table(
    "history",
    metadata,
    Column("id", Integer, primary_key=True, autoincrement=True),
    Column("uuid", Text, nullable=True),
    Column("file", Text, nullable=False),
    Column("time", DateTime, nullable=False),
    Column("profile_name", Text, nullable=False),
    Column("profile_id", String, nullable=False),
    Column("profile_key", Integer, ForeignKey("profile.key"), nullable=False),
    Column("data", Text, nullable=False),
)

stage_fts_table = None
profile_fts_table = None


def decompress_zst_file(zst_file_path):
    with open(zst_file_path, "rb") as compressed_file:
        decompressor = zstd.ZstdDecompressor()
        decompressed_content = decompressor.stream_reader(compressed_file)
        return decompressed_content.read()


def process_json_data(json_data):
    data = json.loads(json_data)

    if "last_changed" not in data:
        data["last_changed"] = 0

    # You can add more processing logic here if needed
    return data


def scan_and_process_files(base_directory):
    for root, dirs, files in os.walk(base_directory):
        for file in files:
            if file.endswith(".json.zst"):
                file_path = os.path.join(root, file)
                try:
                    decompressed_data = decompress_zst_file(file_path)
                    json_data = decompressed_data.decode("utf-8")
                    data = process_json_data(json_data)
                    logging.warning(f"Processed data from {file_path}:")
                    data["file"] = file_path
                    try:
                        Database.insert_history(data)
                    except Exception as e:
                        logging.warning(
                            f"Error inserting shot from file {file_path}: {e.__class__.__name__} {e}",
                            stack_info=True,
                        )
                        logging.warning(traceback.format_exc())
                except Exception as e:
                    logging.warning(
                        f"Error processing file {file_path}: {e.__class__.__name__} {e}"
                    )


class Database:
    @staticmethod
    def init():
        global profile_fts_table
        global stage_fts_table

        try:
            # Ensure tables are created
            metadata.create_all(engine)

            # Ensure FTS tables are created
            with engine.connect() as connection:
                connection.execute(
                    text(
                        "CREATE VIRTUAL TABLE IF NOT EXISTS profile_fts USING fts5(profile_key, profile_id, name)"
                    )
                )
                connection.execute(
                    text(
                        "CREATE VIRTUAL TABLE IF NOT EXISTS stage_fts USING fts5(profile_key, profile_id, profile_name, stage_key, stage_name)"
                    )
                )
                # Register FTS tables with SQLAlchemy
                profile_fts_table = Table("profile_fts", metadata, autoload_with=engine)
                stage_fts_table = Table("stage_fts", metadata, autoload_with=engine)

                Database.enable_compression(connection)
        except sqlite3.DatabaseError as e:
            logging.warning("Database error: %s", e)
            Database.handle_error(e)

        # Read all existing shots from disk
        base_directory = "history/shots"
        scan_and_process_files(base_directory)

    @staticmethod
    def enable_compression(connection):
        try:
            connection.execute(
                text(
                    """
                SELECT zstd_enable_transparent('{
                    "table": "history",
                    "column": "data", 
                    "compression_level": 19,
                    "dict_chooser": "''a''"
                }')
                """
                )
            )
            connection.execute(text("SELECT zstd_incremental_maintenance(60, 0.5)"))
        except sqlalchemy.exc.OperationalError as e:
            if "is already enabled for compression" in str(e):
                pass
            else:
                logging.error("Failure during compression setup: " + str(e))
                raise e

        # Verify the compression
        verify = connection.execute(text("PRAGMA table_info(history)"))
        results = verify.fetchall()
        for row in results:
            logging.warning(dict(row._mapping))

    @staticmethod
    def handle_error(e):
        if "database disk image is malformed" in str(e):
            logging.warning("Database corrupted, reinitializing by deleteing...")
            Database.delete_and_rebuild()
        elif "unable to open database file" in str(e):
            logging.warning(
                "Cannot open database file, attempting to delete and completely rebuild the database..."
            )
            Database.delete_and_rebuild()
        else:
            logging.error("Unhandled database error: %s", e)

    @staticmethod
    def delete_and_rebuild():
        try:
            # Close the engine connection before deleting the file
            engine.dispose()

            # Delete the database file
            if os.path.exists(DATABASE_FILE):
                os.remove(DATABASE_FILE)
                logging.info("Database file deleted successfully.")

            # Recreate the entire database
            Database.init()
        except sqlite3.DatabaseError as e:
            logging.error("Failed to completely rebuild the database: %s", e)
        except OSError as e:
            logging.error("Failed to delete the database file: %s", e)

    @staticmethod
    def profile_exists(profile_data):
        stages_json = json.dumps(profile_data["stages"])
        variables_json = json.dumps(profile_data["variables"])
        previous_authors_json = json.dumps(profile_data["previous_authors"])

        query = (
            select(profile_table.c.key)
            .where(profile_table.c.id == profile_data["id"])
            .where(profile_table.c.author == profile_data["author"])
            .where(profile_table.c.author_id == profile_data["author_id"])
            .where(profile_table.c.display_image == profile_data["display"]["image"])
            .where(profile_table.c.final_weight == profile_data["final_weight"])
            .where(profile_table.c.last_changed == profile_data.get("last_changed", 0))
            .where(profile_table.c.name == profile_data["name"])
            .where(profile_table.c.temperature == profile_data["temperature"])
            .where(
                func.json_extract(profile_table.c.stages, "$")
                == func.json_extract(stages_json, "$")
            )
            .where(
                func.json_extract(profile_table.c.variables, "$")
                == func.json_extract(variables_json, "$")
            )
            .where(
                func.json_extract(profile_table.c.previous_authors, "$")
                == func.json_extract(previous_authors_json, "$")
            )
        )

        with engine.connect() as connection:
            existing_profile = connection.execute(query).fetchone()
            return existing_profile

    @staticmethod
    def insert_profile(profile_data):
        existing_profile = Database.profile_exists(profile_data)

        if existing_profile:
            logging.debug(
                f"Profile with id {profile_data['id']}, name {profile_data['name']}, and author_id {profile_data['author_id']} already exists."
            )
            return existing_profile[0]
        with engine.connect() as connection:
            with connection.begin():
                ins_stmt = insert(profile_table).values(
                    id=profile_data["id"],
                    author=profile_data["author"],
                    author_id=profile_data["author_id"],
                    display_image=profile_data["display"]["image"],
                    final_weight=profile_data["final_weight"],
                    last_changed=profile_data.get("last_changed", 0),
                    name=profile_data["name"],
                    temperature=profile_data["temperature"],
                    stages=profile_data["stages"],
                    variables=profile_data["variables"],
                    previous_authors=profile_data["previous_authors"],
                )
                result = connection.execute(ins_stmt)
                profile_key = result.inserted_primary_key[0]

                # Insert into profile FTS table
                fts_ins_stmt = insert(profile_fts_table).values(
                    profile_key=profile_key,
                    profile_id=profile_data["id"],
                    name=profile_data["name"],
                )
                connection.execute(fts_ins_stmt)

                # Insert stages into stage_fts
                for stage in profile_data["stages"]:
                    stage_fts_ins_stmt = insert(stage_fts_table).values(
                        profile_key=profile_key,
                        profile_id=profile_data["id"],
                        profile_name=profile_data["name"],
                        stage_key=stage["key"],
                        stage_name=stage["name"],
                    )
                    connection.execute(stage_fts_ins_stmt)
                return profile_key

    @staticmethod
    def history_exists(entry):
        query = select(history_table.c.id).where(history_table.c.file == entry["file"])

        with engine.connect() as connection:
            existing_history = connection.execute(query).fetchone()
            return existing_history

    @staticmethod
    def insert_history(entry):
        if "id" not in entry:
            entry["id"] = str(uuid.uuid4())
        existing_history = Database.history_exists(entry)

        if existing_history:
            logging.debug(f"History entry with file {entry['file']} already exists.")
            return existing_history[0]

        profile_data = entry["profile"]
        profile_key = Database.insert_profile(profile_data)
        with engine.connect() as connection:
            with connection.begin():

                # Convert to UTC
                time_obj = datetime.strptime(entry["time"], "%Y_%m_%d_%H_%M_%S")
                time_obj = pytz.timezone("UTC").localize(time_obj)

                ins_stmt = insert(history_table).values(
                    uuid=entry["id"],
                    file=entry["file"],
                    time=time_obj,
                    profile_name=entry["profile_name"],
                    profile_id=profile_data["id"],
                    profile_key=profile_key,
                    data=json.dumps(entry["data"]),
                )
                connection.execute(ins_stmt)

    @staticmethod
    def delete_shot(shot_id):
        with engine.connect() as connection:
            with connection.begin():
                # Delete from history
                del_stmt = delete(history_table).where(history_table.c.id == shot_id)
                connection.execute(del_stmt)

                # Get the profile_key of the deleted shot
                profile_key_stmt = select([history_table.c.profile_key]).where(
                    history_table.c.id == shot_id
                )
                connection.execute(profile_key_stmt).fetchone()

                # Check for orphaned profiles
                orphaned_profiles_stmt = select([profile_table.c.key]).where(
                    ~profile_table.c.key.in_(select([history_table.c.profile_key]))
                )
                orphaned_profiles = connection.execute(
                    orphaned_profiles_stmt
                ).fetchall()
                for orphan in orphaned_profiles:
                    del_profile_stmt = delete(profile_table).where(
                        profile_table.c.key == orphan[0]
                    )
                    connection.execute(del_profile_stmt)

                    # Delete from profile_fts
                    del_profile_fts_stmt = delete(profile_fts_table).where(
                        profile_fts_table.c.key == orphan[0]
                    )
                    connection.execute(del_profile_fts_stmt)

                    # Delete stages from stage_fts
                    del_stage_fts_stmt = delete(stage_fts_table).where(
                        stage_fts_table.c.profile_key == orphan[0]
                    )
                    connection.execute(del_stage_fts_stmt)

    @staticmethod
    def search_history(
        search_query=None,
        date_start=None,
        date_end=None,
        group_by=None,
        page=None,
        per_page=50,
    ):
        stmt = (
            select(
                history_table.c.uuid.label("history_uuid"),
                history_table.c.id,
                history_table.c.file,
                history_table.c.time,
                history_table.c.profile_name,
                history_table.c.profile_id,
                history_table.c.profile_key,
                profile_table.c.author,
                profile_table.c.author_id,
                profile_table.c.display_image,
                profile_table.c.final_weight,
                profile_table.c.last_changed,
                profile_table.c.temperature,
                profile_table.c.stages,
                profile_table.c.variables,
                profile_table.c.previous_authors,
            )
            .distinct()
            .select_from(
                history_table.join(
                    profile_table, history_table.c.profile_key == profile_table.c.key
                )
            )
            .outerjoin(
                profile_fts_table,
                profile_table.c.key == profile_fts_table.c.profile_key,
            )
            .outerjoin(
                stage_fts_table, profile_table.c.key == stage_fts_table.c.profile_key
            )
        )

        if search_query:
            stmt = stmt.where(
                or_(
                    profile_fts_table.c.name.like(f"%{search_query}%"),
                    stage_fts_table.c.stage_name.like(f"%{search_query}%"),
                )
            )

        if date_start:
            stmt = stmt.where(history_table.c.time >= date_start)

        if date_end:
            stmt = stmt.where(history_table.c.time <= date_end)

        if group_by == "day":
            stmt = stmt.order_by(history_table.c.time)
        elif group_by == "profile":
            stmt = stmt.order_by(history_table.c.profile_name)
        elif group_by == "both":
            stmt = stmt.order_by(history_table.c.profile_name, history_table.c.time)

        if page is not None and per_page is not None:
            offset = (page - 1) * per_page
            stmt = stmt.limit(per_page).offset(offset)

        with engine.connect() as connection:
            results = connection.execute(stmt).fetchall()
            logging.warning(f"total results = {len(results)}")
            results_dicts = [
                {
                    "history_id": row.history_uuid,
                    "dbkey": row.history_id,
                    "file": row.file,
                    "time": row.time,
                    "profile_name": row.profile_name,
                    "profile_id": row.profile_id,
                    "profile_key": row.profile_key,
                    "author": row.author,
                    "author_id": row.author_id,
                    "display_image": row.display_image,
                    "final_weight": row.final_weight,
                    "last_changed": row.last_changed,
                    "temperature": row.temperature,
                    "stages": row.stages,
                    "variables": row.variables,
                    "previous_authors": row.previous_authors,
                }
                for row in results
            ]
            return results_dicts

    @staticmethod
    def autocomplete_profile_name(prefix):
        with Session() as session:
            if not prefix:
                stmt = (
                    select(history_table.c.profile_name)
                    .distinct()
                    .group_by(history_table.c.profile_name)
                    .order_by(func.count().desc())
                )
                results = session.execute(stmt).fetchall()
                return [{"profile": result[0], "type": "profile"} for result in results]

            # Update queries to use LIKE instead of MATCH for partial matching
            stmt_profile = (
                select(profile_fts_table.c.name.label("name"))
                .distinct()
                .where(profile_fts_table.c.name.like(f"%{prefix}%"))
            )

            stmt_stage = (
                select(stage_fts_table.c.profile_name, stage_fts_table.c.stage_name)
                .distinct()
                .where(stage_fts_table.c.stage_name.like(f"%{prefix}%"))
            )

            profile_results = session.execute(stmt_profile).fetchall()
            stage_results = session.execute(stmt_stage).fetchall()

            results = [
                {"profile": res.name, "type": "profile"} for res in profile_results
            ]
            for result in stage_results:
                results.append(
                    {
                        "profile": result.profile_name,
                        "type": "stage",
                        "name": result.stage_name,
                    }
                )

            return results

    @staticmethod
    def get_shots(history_ids):
        stmt = (
            select(
                *[c.label(f"history_{c.name}") for c in history_table.c],
                *[c.label(f"profile_{c.name}") for c in profile_table.c],
            )
            .select_from(
                history_table.join(
                    profile_table, history_table.c.profile_key == profile_table.c.key
                )
            )
            .where(
                or_(
                    history_table.c.id.in_(history_ids),
                    history_table.c.uuid.in_(history_ids),
                )
            )
        )

        with engine.connect() as connection:
            results = connection.execute(stmt)
            parsed_results = []
            for row in results:
                row_dict = dict(row._mapping)

                profile = {
                    "id": row_dict.pop("profile_id"),
                    "db_key": row_dict.pop("profile_key"),
                    "author": row_dict.pop("profile_author"),
                    "author_id": row_dict.pop("profile_author_id"),
                    "display_image": row_dict.pop("profile_display_image"),
                    "final_weight": row_dict.pop("profile_final_weight"),
                    "last_changed": row_dict.pop("profile_last_changed"),
                    "name": row_dict.pop("profile_name"),
                    "temperature": row_dict.pop("profile_temperature"),
                    "stages": row_dict.pop("profile_stages"),
                    "variables": row_dict.pop("profile_variables"),
                    "previous_authors": row_dict.pop("profile_previous_authors"),
                }
                history = {
                    "id": row_dict.pop("history_uuid"),
                    "db_key": row_dict.pop("history_id"),
                    "time": row_dict.pop("history_time"),
                    "file": row_dict.pop("history_file"),
                    "name": row_dict.pop("history_profile_name"),
                    "data": json.loads(row_dict["history_data"]),
                    # "profile_id": row_dict.pop("history_profile_id"),
                    # "profile_db_key": row_dict.pop("history_profile_key"),
                    "profile": profile,
                }

                parsed_results.append(history)

            return parsed_results


app = Flask(__name__)


@app.route("/compare", methods=["GET"])
def compare():
    history_ids = request.args.get("history_ids")
    if not history_ids:
        return jsonify([])

    history_ids = history_ids.split(",")
    results = Database.get_shots(history_ids)
    return jsonify(results)


@app.route("/search", methods=["GET"])
def search():
    query = request.args.get("query")
    date_start = request.args.get("date_start")
    date_end = request.args.get("date_end")
    group_by = request.args.get("group_by")
    page = request.args.get("page", type=int)
    per_page = request.args.get("per_page", type=int, default=50)
    logging.warning(f"searching for {query}")
    results = Database.search_history(
        search_query=query,
        date_start=date_start,
        date_end=date_end,
        group_by=group_by,
        page=page,
        per_page=per_page,
    )
    return jsonify(results)


@app.route("/autocomplete", methods=["GET"])
def autocomplete():
    prefix = request.args.get("prefix")
    results = Database.autocomplete_profile_name(prefix)
    return jsonify(results)


# Minimal HTML page
html_page = """
<!DOCTYPE html>
<html>
<head>
    <title>Search and Autocomplete Demo</title>
    <style>
    canvas{

    width:1000px !important;
    height:600px !important;

    }

    #compare-results table {
        width: 100%;
        border-collapse: collapse;
        margin-top: 20px;
    }
    #compare-results th, #compare-results td {
        border: 1px solid #ccc;
        padding: 8px;
        text-align: left;
    }
    #compare-results th {
        background-color: #f2f2f2;
    }
    #compare-chart {
        margin-top: 20px;
    }
    #autocomplete-container {
        position: relative;
    }
    #autocomplete-results {
        position: absolute;
        top: 100%;
        left: 0;
        right: 0;
        border: 1px solid #ccc;
        border-top: none;
        overflow-y: auto;
        background: white;
        z-index: 1000;
        display: none;
    }
    #autocomplete-results li {
        padding: 8px;
        cursor: pointer;
        display: flex;
        flex-direction: column;
        justify-content: center;
        height: 50px;
    }
    #autocomplete-results li.highlighted {
        background-color: #ddd;
    }

    .stage-name {
        font-size: 12px;
        color: grey;
    }
    #search-results {
        display: flex;
        flex-direction: column;
        gap: 20px;
    }
    .profile-group {
        width: 100%;
    }
    .profile-headline {
        font-size: 24px;
        margin-bottom: 10px;
        color: grey;
    }
    .results {
        display: flex;
        flex-direction: row;
        flex-wrap: wrap;
    }
    .result-item {
        display: flex;
        flex-direction: column;
        align-items: center;
        text-align: center;
        width: 220px; /* Fixed size */
        padding: 0 5px;
    }
    .result-item img {
        width: 200px;
        height: 200px;
        border: 10px solid white;
        box-shadow: 0 4px 8px rgba(0, 0, 0, 0.2);
        margin-bottom: 10px;
    }
    .result-item .profile-name,
    .result-item .profile-date,
    .result-item .profile-id {
        padding-left: 10px;
        padding-right: 10px;
        font-size: 14px;
        color: grey;
        word-wrap: break-word; /* Allow text to break */
        width: 100%; /* Ensure text takes full width */
        box-sizing: border-box; /* Include padding/border in element's width/height */
    }

    </style>
    <script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
</head>
<body>
    <h1>Search and Autocomplete Demo</h1>
    <h2>Compare Histories</h2>
        <form id="compare-form">
            <label for="history_ids">History IDs (comma separated):</label>
            <input type="text" id="history_ids" name="history_ids"><br><br>

            <input type="button" value="Compare" onclick="performCompare()">
        </form>
        <div class="chart-container">
            <canvas id="combined-chart"></canvas>
        </div>

        <div id="compare-results"></div>

    <h2>Search</h2>
    <form id="search-form">

        <label for="date_start">Start Date:</label>
        <input type="date" id="date_start" name="date_start"><br><br>

        <label for="date_end">End Date:</label>
        <input type="date" id="date_end" name="date_end"><br><br>
        
        <label for="group_by">Group By:</label>
        <select id="group_by" name="group_by">
            <option value="none">None</option>
            <option value="day">Day</option>
            <option value="profile">Profile</option>
            <option value="both">Both</option>
        </select><br><br>
        <label for="query">Profile Name:</label>
        <input type="text" id="query" name="query" autocomplete="off" oninput="performAutocomplete()" onfocus="performAutocomplete()"><br><br>
        
        <div id="autocomplete-container">
            <ul id="autocomplete-results"></ul>
        </div>


        <input type="button" value="Search" onclick="performSearch()">
    </form>
    <div id="search-results"></div>
    
    <script>
        function performSearch() {
            const form = document.getElementById('search-form');
            const formData = new FormData(form);
            const params = new URLSearchParams(formData);

            fetch('/search?' + params.toString())
                .then(response => response.json())
                .then(data => {
                    
                    const resultsContainer = document.getElementById('search-results');
                    resultsContainer.innerHTML = ''; // Clear previous results

                    const groupBy = formData.get('group_by');
                    let currentGroup = null;
                    let profileContainer = null;
                    let results = null;

                    data.forEach(item => {
                        let itemGroup = '';
                        var headline = ''
                        if (groupBy === 'profile') {
                            itemGroup = item.profile_id;
                            headline = item.profile_name;
                        } else if (groupBy === 'day') {
                            itemGroup = new Date(item.time).toLocaleDateString();
                            headline = itemGroup;
                        } else if (groupBy === 'both') {
                            const day = new Date(item.time).toLocaleDateString();
                            itemGroup = item.profile_id + day;
                            headline = `${item.profile_name} : ${day}`;
                        } else {
                            itemGroup = 'None';
                        }
                        if (itemGroup !== currentGroup) {
                            currentGroup = itemGroup;
                            currentProfile = item.profile_id;

                            // Create a new container for the current profile group
                            profileContainer = document.createElement('div');
                            profileContainer.className = 'profile-group';

                            // Create a headline for the current profile
                            const profileHeadline = document.createElement('h2');
                            profileHeadline.className = 'profile-headline';
                            profileHeadline.textContent = headline;
                            profileContainer.appendChild(profileHeadline);
                            
                                                    
                            results = document.createElement('div');
                            results.className = 'results';
                            profileContainer.appendChild(results);

                            resultsContainer.appendChild(profileContainer);
                        }

                        const resultItem = document.createElement('div');
                        resultItem.className = 'result-item';
                resultItem.onmouseover = () => {
                    resultItem.style.transform = 'scale(1.05)';
                };

                resultItem.onmouseout = () => {
                    resultItem.style.transform = 'scale(1)';
                };

                resultItem.onclick = () => {
                    window.scrollTo({ top: 0, behavior: 'smooth' });
                    addToCompare(item.dbkey);
                };
                        const img = document.createElement('img');
                        var url = "http://localhost:8080";
                        if (!item.display_image.startsWith("/api/"))
                            url += "/api/v1/profile/image/";
                        img.src = url + item.display_image;
                        resultItem.appendChild(img);

                        const profileName = document.createElement('div');
                        profileName.className = 'profile-name';
                        profileName.textContent = item.profile_name;
                        resultItem.appendChild(profileName);

                        const profileDate = document.createElement('div');
                        profileDate.className = 'profile-date';
                        profileDate.textContent = new Date(item.time).toLocaleString();
                        resultItem.appendChild(profileDate);

                        const profileId = document.createElement('div');
                        profileId.className = 'profile-id';
                        profileId.textContent = 'history ID: ' + item.dbkey;
                        resultItem.appendChild(profileId);

                        results.appendChild(resultItem);
                    });
                    
                });
        }
        function addToCompare(number) {
            const compareForm = document.getElementById('compare-form');
            const input = compareForm.querySelector('input[name="history_ids"]');
            let currentValue = input.value ? input.value.split(',') : [];

            if (!currentValue.includes(String(number))) {
                currentValue.push(number);
                input.value = currentValue.join(',');
                performCompare();
            }
        }

        let currentIndex = 0;

        function performAutocomplete() {
            const prefix = document.getElementById('query').value;
            
            fetch('/autocomplete?prefix=' + encodeURIComponent(prefix))
                .then(response => response.json())
                .then(data => {
                    const resultsContainer = document.getElementById('autocomplete-results');
                    resultsContainer.style.display="block"
                    resultsContainer.innerHTML = ''; // Clear previous results
                    currentIndex = 0;
                    
                    // Add the current content of the profile name field into the dropdown

                    const currentInputItem = document.createElement('li');
                    const currentInputText = document.createElement('span');
                    currentInputText.textContent = prefix;
                    currentInputItem.appendChild(currentInputText);
                    currentInputItem.onclick = () => {
                        document.getElementById('query').value = prefix;
                        resultsContainer.innerHTML = ''; // Clear results after selection
                        resultsContainer.style.display="none"

                        performSearch();
                    };
                    resultsContainer.appendChild(currentInputItem);
                    
                    data.forEach((item, index) => {
                        const listItem = document.createElement('li');
                        const profileName = document.createElement('span');
                        profileName.textContent = item.profile;
                        listItem.appendChild(profileName);
                        
                        if (item.type === 'stage') {
                            const stageName = document.createElement('span');
                            stageName.textContent = `stage: ${item.name}`;
                            stageName.className = 'stage-name';
                            listItem.appendChild(stageName);
                        }
                        
                        listItem.onclick = () => {
                            document.getElementById('query').value = item.profile;
                            resultsContainer.innerHTML = ''; // Clear results after selection
                            resultsContainer.style.display="none"
                            performSearch();
                        };
                        resultsContainer.appendChild(listItem);
                    });
                    const items = resultsContainer.getElementsByTagName('li');
                    updateHighlight(items)
                });
        }

        document.getElementById('query').addEventListener('keydown', function(event) {
            console.log(event)
            const resultsContainer = document.getElementById('autocomplete-results');
            const items = resultsContainer.getElementsByTagName('li');
            updateHighlight(items)
            if (event.key === 'ArrowDown') {
                currentIndex = (currentIndex + 1) % items.length;
                updateHighlight(items);
            } else if (event.key === 'ArrowUp') {
                currentIndex = (currentIndex - 1 + items.length) % items.length;
                updateHighlight(items);
            } else if (event.key === 'Enter') {
                event.preventDefault(); // Prevent form submission
                if (currentIndex > -1 && items.length > currentIndex) {
                    items[currentIndex].click();
                } else {
                    performSearch();
                }
            } 
        });

        function updateHighlight(items) {
            for (let i = 0; i < items.length; i++) {
                items[i].classList.remove('highlighted');
            }
            if (currentIndex > -1 && items.length > currentIndex) {
                console.log(items.length)
                console.log(currentIndex)
                items[currentIndex].classList.add('highlighted');
                items[currentIndex].scrollIntoView({ block: 'nearest' });
            }
        }

        function performCompare() {
            const form = document.getElementById('compare-form');
            const formData = new FormData(form);
            const params = new URLSearchParams(formData);
            
            fetch('/compare?' + params.toString())
                .then(response => response.json())
                .then(data => {
                    const resultsContainer = document.getElementById('compare-results');
                    resultsContainer.innerHTML = ''; // Clear previous results
                    const table = document.createElement('table');
                    const thead = document.createElement('thead');
                    const tbody = document.createElement('tbody');

                    // Create table headers
                    const headers = ['History ID', 'History DB Key', 'Profile Name', 'Profile ID', 'Time'];
                    const headerRow = document.createElement('tr');
                    headers.forEach(header => {
                        const th = document.createElement('th');
                        th.textContent = header;
                        headerRow.appendChild(th);
                    });
                    thead.appendChild(headerRow);
                    table.appendChild(thead);

                    const compareDataPoints = [];

                    // Create table rows
                    data.forEach(item => {
                        
                            const row = document.createElement('tr');
                            row.innerHTML = `
                                <td>${item.id}</td>
                                <td>${item.db_key}</td>
                                <td>${item.profile.name}</td>
                                <td>${item.profile.id}</td>
                                <td>${new Date(item.time).toLocaleString()}</td>
                            `;
                            tbody.appendChild(row);
                    
                    });

                    table.appendChild(tbody);
                    resultsContainer.appendChild(table);
                    
                    data.forEach(item => {
                        const offset = Math.random();
                        const dataPoints = {
                            label: item.profile.name,
                            data: {
                                pressure: [],
                                flow: [],
                                weight: [],
                                temperature: []
                            }
                        };
                        item.data.forEach(entry => {
                            dataPoints.data.pressure.push({ x: entry.time, y: entry.shot.pressure*(offset*8) });
                            dataPoints.data.flow.push({ x: entry.time, y: entry.shot.flow*(offset*8) });
                            dataPoints.data.weight.push({ x: entry.time, y: entry.shot.weight*(offset*8)     });
                            dataPoints.data.temperature.push({ x: entry.time, y: entry.shot.temperature*offset });
                        });
                        compareDataPoints.push(dataPoints);
                    });

                    renderCombinedChart(compareDataPoints);

                });
        }
        document.getElementById('history_ids').addEventListener('keydown', function(event) {
            if (event.key === 'Enter' && document.getElementById('autocomplete-results').style.display === 'none') {
                event.preventDefault();
                performSearch();
            }
        });
        function getRandomColor() {
            const letters = '0123456789ABCDEF';
            let color = '#';
            for (let i = 0; i < 6; i++) {
                color += letters[Math.floor(Math.random() * 16)];
            }
            return color;
        }

        var combinedChart = null;
        function renderCombinedChart(dataSets) {
            const combinedCanvas = document.getElementById('combined-chart');

            const datasets = [];

            dataSets.forEach(dataSet => {
                datasets.push(
                    {
                        label: `${dataSet.label} - Pressure`,
                        data: dataSet.data.pressure,
                        borderColor: getRandomColor(),
                        fill: false,
                    },
                    {
                        label: `${dataSet.label} - Flow`,
                        data: dataSet.data.flow,
                        borderColor: getRandomColor(),
                        fill: false,
                    },
                    {
                        label: `${dataSet.label} - Weight`,
                        data: dataSet.data.weight,
                        borderColor: getRandomColor(),
                        fill: false,
                    },
                    {
                        label: `${dataSet.label} - Temperature`,
                        data: dataSet.data.temperature,
                        borderColor: getRandomColor(),
                        fill: false,
                    }
                );
            });

        if (combinedChart) {
            combinedChart.data.datasets = datasets;
            combinedChart.update();
        } else {
            combinedChart = new Chart(combinedCanvas.getContext('2d'), {
                type: 'line',
                data: {
                    datasets: datasets
                },
                options: {
                    scales: {
                        x: {
                            type: 'linear',
                            position: 'bottom'
                        }
                    },
                    elements: {
                        point:{
                            radius: 0
                        }
                    },
                    responsive: true,
                    maintainAspectRatio: true
                }
            });
            }
        }

    </script>
</body>
</html> 

"""


@app.route("/")
def index():
    return render_template_string(html_page)


if __name__ == "__main__":
    Database.init()

    app.run(debug=True, host="0.0.0.0")
