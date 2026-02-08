import json
import os
import time
import tempfile
from pathlib import Path

import zstandard as zstd

import pytest

import config as cfg
import shot_database as sdb_module
from shot_database import ShotDataBase, SearchParams, SearchOrder, SearchOrderBy
from database_models import metadata


@pytest.fixture(autouse=True)
def shot_db(tmp_path, monkeypatch):
    db_file = tmp_path / "history.sqlite"
    db_url = f"sqlite:///{db_file}"

    # Patch both the config module and the shot_database module's local bindings
    monkeypatch.setattr(cfg, "HISTORY_PATH", str(tmp_path))
    monkeypatch.setattr(cfg, "ABSOLUTE_DATABASE_FILE", db_file)
    monkeypatch.setattr(cfg, "DATABASE_URL", db_url)
    monkeypatch.setattr(cfg, "SHOT_PATH", tmp_path / "shots")

    monkeypatch.setattr(sdb_module, "HISTORY_PATH", str(tmp_path))
    monkeypatch.setattr(sdb_module, "ABSOLUTE_DATABASE_FILE", db_file)
    monkeypatch.setattr(sdb_module, "DATABASE_URL", db_url)
    monkeypatch.setattr(sdb_module, "SHOT_PATH", tmp_path / "shots")

    # Reset class-level state so init() starts fresh
    ShotDataBase.engine = None
    ShotDataBase.session = None
    ShotDataBase.stage_fts_table = None
    ShotDataBase.profile_fts_table = None

    # Clear any cached FTS table references from metadata
    for table_name in list(metadata.tables.keys()):
        if table_name in ("profile_fts", "stage_fts"):
            metadata.remove(metadata.tables[table_name])

    ShotDataBase.init()
    metadata.create_all(ShotDataBase.engine)
    yield ShotDataBase

    if ShotDataBase.engine:
        ShotDataBase.engine.dispose()


def make_profile(**overrides):
    base = {
        "id": "profile-001",
        "author": "Test Author",
        "author_id": "author-001",
        "display": {},
        "final_weight": 36,
        "last_changed": 0,
        "name": "Test Espresso",
        "temperature": 93,
        "stages": [
            {"key": "stage-1", "name": "Preinfusion", "type": "pressure"},
            {"key": "stage-2", "name": "Extraction", "type": "pressure"},
        ],
        "variables": [],
        "previous_authors": [],
    }
    base.update(overrides)
    return base


def make_history_entry(profile=None, **overrides):
    if profile is None:
        profile = make_profile()
    base = {
        "id": overrides.pop("id", "hist-001"),
        "file": overrides.pop("file", "shot_001.json.zst"),
        "time": overrides.pop("time", time.time()),
        "profile_name": profile["name"],
        "profile": profile,
    }
    base.update(overrides)
    return base


class TestInsertProfile:
    def test_insert_returns_key(self):
        profile = make_profile()
        key = ShotDataBase.insert_profile(profile)
        assert key is not None
        assert isinstance(key, int)
        assert key > 0

    def test_insert_same_profile_returns_same_key(self):
        profile = make_profile()
        key1 = ShotDataBase.insert_profile(profile)
        key2 = ShotDataBase.insert_profile(profile)
        assert key1 == key2

    def test_insert_different_profiles_return_different_keys(self):
        p1 = make_profile(id="p1", name="Profile A")
        p2 = make_profile(id="p2", name="Profile B")
        key1 = ShotDataBase.insert_profile(p1)
        key2 = ShotDataBase.insert_profile(p2)
        assert key1 != key2

    def test_insert_none_returns_negative(self):
        result = ShotDataBase.insert_profile(None)
        assert result == -1


class TestInsertHistory:
    def test_insert_returns_id(self):
        entry = make_history_entry()
        history_id = ShotDataBase.insert_history(entry)
        assert history_id is not None
        assert isinstance(history_id, int)

    def test_insert_same_file_returns_same_id(self):
        entry = make_history_entry(file="same.json.zst")
        id1 = ShotDataBase.insert_history(entry)
        id2 = ShotDataBase.insert_history(entry)
        assert id1 == id2

    def test_insert_different_files_return_different_ids(self):
        e1 = make_history_entry(id="h1", file="shot_a.json.zst")
        e2 = make_history_entry(id="h2", file="shot_b.json.zst")
        id1 = ShotDataBase.insert_history(e1)
        id2 = ShotDataBase.insert_history(e2)
        assert id1 != id2


class TestSearchHistory:
    def test_search_returns_inserted_entry(self):
        entry = make_history_entry()
        ShotDataBase.insert_history(entry)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["name"] == "Test Espresso"
        assert results[0]["profile"]["name"] == "Test Espresso"

    def test_search_with_max_results(self):
        for i in range(5):
            e = make_history_entry(
                id=f"h{i}",
                file=f"shot_{i}.json.zst",
                time=time.time() + i,
            )
            ShotDataBase.insert_history(e)

        params = SearchParams(max_results=3, dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 3

    def test_search_by_profile_id(self):
        p1 = make_profile(id="p-aaa", name="Ristretto")
        p2 = make_profile(id="p-bbb", name="Lungo")
        e1 = make_history_entry(profile=p1, id="h1", file="s1.zst")
        e2 = make_history_entry(profile=p2, id="h2", file="s2.zst")
        ShotDataBase.insert_history(e1)
        ShotDataBase.insert_history(e2)

        params = SearchParams(ids=["p-aaa"], dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["profile"]["id"] == "p-aaa"

    def test_search_ascending_order(self):
        for i in range(3):
            e = make_history_entry(
                id=f"h{i}",
                file=f"s{i}.zst",
                time=1000000 + i * 100,
            )
            ShotDataBase.insert_history(e)

        params = SearchParams(
            sort=SearchOrder.ascending,
            dump_data=False,
        )
        results = ShotDataBase.search_history(params)
        times = [r["time"] for r in results]
        assert times == sorted(times)

    def test_search_descending_order(self):
        for i in range(3):
            e = make_history_entry(
                id=f"h{i}",
                file=f"s{i}.zst",
                time=1000000 + i * 100,
            )
            ShotDataBase.insert_history(e)

        params = SearchParams(
            sort=SearchOrder.descending,
            dump_data=False,
        )
        results = ShotDataBase.search_history(params)
        times = [r["time"] for r in results]
        assert times == sorted(times, reverse=True)

    def test_search_by_query_matches_profile_name(self):
        p1 = make_profile(id="p1", name="Morning Espresso")
        p2 = make_profile(id="p2", name="Evening Lungo")
        e1 = make_history_entry(profile=p1, id="h1", file="s1.zst")
        e2 = make_history_entry(profile=p2, id="h2", file="s2.zst")
        ShotDataBase.insert_history(e1)
        ShotDataBase.insert_history(e2)

        params = SearchParams(query="Morning", dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["profile"]["name"] == "Morning Espresso"

    def test_search_by_query_matches_stage_name(self):
        p = make_profile(
            id="p1",
            name="Custom Profile",
            stages=[
                {"key": "s1", "name": "Bloom Phase", "type": "pressure"},
            ],
        )
        e = make_history_entry(profile=p, id="h1", file="s1.zst")
        ShotDataBase.insert_history(e)

        params = SearchParams(query="Bloom", dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1

    def test_search_empty_database(self):
        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert results == []


class TestRateShot:
    def _insert_shot(self):
        entry = make_history_entry()
        ShotDataBase.insert_history(entry)
        return entry["id"]

    def test_rate_like(self):
        uuid = self._insert_shot()
        result = ShotDataBase.rate_shot(uuid, "like")
        assert result is True
        assert ShotDataBase.get_shot_rating(uuid) == "like"

    def test_rate_dislike(self):
        uuid = self._insert_shot()
        result = ShotDataBase.rate_shot(uuid, "dislike")
        assert result is True
        assert ShotDataBase.get_shot_rating(uuid) == "dislike"

    def test_change_rating(self):
        uuid = self._insert_shot()
        ShotDataBase.rate_shot(uuid, "like")
        ShotDataBase.rate_shot(uuid, "dislike")
        assert ShotDataBase.get_shot_rating(uuid) == "dislike"

    def test_remove_rating(self):
        uuid = self._insert_shot()
        ShotDataBase.rate_shot(uuid, "like")
        ShotDataBase.rate_shot(uuid, None)
        assert ShotDataBase.get_shot_rating(uuid) is None

    def test_rate_nonexistent_shot(self):
        result = ShotDataBase.rate_shot("nonexistent-uuid", "like")
        assert result is False

    def test_invalid_rating_rejected(self):
        uuid = self._insert_shot()
        result = ShotDataBase.rate_shot(uuid, "stars")
        assert result is False

    def test_get_rating_unrated_shot(self):
        uuid = self._insert_shot()
        assert ShotDataBase.get_shot_rating(uuid) is None


class TestStatistics:
    def test_empty_database(self):
        stats = ShotDataBase.statistics()
        assert stats["totalSavedShots"] == 0
        assert stats["byProfile"] == []

    def test_counts_shots_per_profile(self):
        p1 = make_profile(id="p1", name="Espresso")
        p2 = make_profile(id="p2", name="Lungo")
        for i in range(3):
            e = make_history_entry(
                profile=p1, id=f"h-esp-{i}", file=f"esp_{i}.zst"
            )
            ShotDataBase.insert_history(e)
        for i in range(2):
            e = make_history_entry(
                profile=p2, id=f"h-lng-{i}", file=f"lng_{i}.zst"
            )
            ShotDataBase.insert_history(e)

        stats = ShotDataBase.statistics()
        assert stats["totalSavedShots"] == 5
        names = {p["name"] for p in stats["byProfile"]}
        assert names == {"Espresso", "Lungo"}


class TestAutocompleteProfileName:
    def test_no_prefix_returns_all(self):
        p1 = make_profile(id="p1", name="Espresso")
        p2 = make_profile(id="p2", name="Lungo")
        ShotDataBase.insert_history(
            make_history_entry(profile=p1, id="h1", file="s1.zst")
        )
        ShotDataBase.insert_history(
            make_history_entry(profile=p2, id="h2", file="s2.zst")
        )

        results = ShotDataBase.autocomplete_profile_name("")
        profile_names = {r["profile"] for r in results}
        assert "Espresso" in profile_names
        assert "Lungo" in profile_names

    def test_prefix_filters(self):
        p1 = make_profile(id="p1", name="Espresso")
        p2 = make_profile(id="p2", name="Lungo")
        ShotDataBase.insert_history(
            make_history_entry(profile=p1, id="h1", file="s1.zst")
        )
        ShotDataBase.insert_history(
            make_history_entry(profile=p2, id="h2", file="s2.zst")
        )

        results = ShotDataBase.autocomplete_profile_name("Esp")
        profile_names = [r["profile"] for r in results]
        assert "Espresso" in profile_names
        assert "Lungo" not in profile_names

    def test_stage_name_match(self):
        p = make_profile(
            id="p1",
            name="Custom",
            stages=[{"key": "s1", "name": "Bloom", "type": "pressure"}],
        )
        ShotDataBase.insert_history(
            make_history_entry(profile=p, id="h1", file="s1.zst")
        )

        results = ShotDataBase.autocomplete_profile_name("Bloom")
        assert any(r.get("type") == "stage" for r in results)


class TestLinkDebugFile:
    def test_link_and_search(self):
        entry = make_history_entry()
        history_id = ShotDataBase.insert_history(entry)

        ShotDataBase.link_debug_file(history_id, "2024-01-01/12:00:00.shot.json.zst")

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert results[0]["debug_file"] == "2024-01-01/12:00:00.shot.json.zst"

    def test_unlink_debug_file(self):
        entry = make_history_entry()
        history_id = ShotDataBase.insert_history(entry)
        debug_path = "2024-01-01/12:00:00.shot.json.zst"

        ShotDataBase.link_debug_file(history_id, debug_path)
        ShotDataBase.unlink_debug_file(debug_path)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert results[0]["debug_file"] is None


def _write_compressed_shot(shot_path, filename, shot_data):
    filepath = Path(shot_path) / filename
    filepath.parent.mkdir(parents=True, exist_ok=True)
    compressor = zstd.ZstdCompressor()
    with open(filepath, "wb") as f:
        f.write(compressor.compress(json.dumps(shot_data).encode()))


class TestSearchHistoryDumpData:
    def test_dump_data_reads_compressed_file(self, tmp_path):
        shot_path = tmp_path / "shots"
        shot_data = {"data": [{"time": 0.1, "pressure": 9.0, "flow": 2.5}]}
        _write_compressed_shot(shot_path, "shot_001.json.zst", shot_data)

        entry = make_history_entry(file="shot_001.json.zst")
        ShotDataBase.insert_history(entry)

        params = SearchParams(dump_data=True)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["data"] == shot_data["data"]

    def test_dump_data_false_returns_none(self):
        entry = make_history_entry()
        ShotDataBase.insert_history(entry)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["data"] is None


class TestSearchHistoryDateFilters:
    def test_start_date_filter(self):
        t_old = 1000000.0
        t_new = 2000000.0
        e_old = make_history_entry(id="h-old", file="old.zst", time=t_old)
        e_new = make_history_entry(id="h-new", file="new.zst", time=t_new)
        ShotDataBase.insert_history(e_old)
        ShotDataBase.insert_history(e_new)

        params = SearchParams(start_date=1500000.0, dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["id"] == "h-new"

    def test_end_date_filter(self):
        t_old = 1000000.0
        t_new = 2000000.0
        e_old = make_history_entry(id="h-old", file="old.zst", time=t_old)
        e_new = make_history_entry(id="h-new", file="new.zst", time=t_new)
        ShotDataBase.insert_history(e_old)
        ShotDataBase.insert_history(e_new)

        params = SearchParams(end_date=1500000.0, dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["id"] == "h-old"

    def test_date_range_filter(self):
        times = [1000000.0, 1500000.0, 2000000.0]
        for i, t in enumerate(times):
            e = make_history_entry(id=f"h{i}", file=f"s{i}.zst", time=t)
            ShotDataBase.insert_history(e)

        params = SearchParams(
            start_date=1200000.0, end_date=1800000.0, dump_data=False
        )
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["id"] == "h1"


class TestSearchHistoryByIds:
    def test_search_by_history_uuid(self):
        e1 = make_history_entry(id="uuid-aaa", file="s1.zst")
        e2 = make_history_entry(id="uuid-bbb", file="s2.zst")
        ShotDataBase.insert_history(e1)
        ShotDataBase.insert_history(e2)

        params = SearchParams(ids=["uuid-aaa"], dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["id"] == "uuid-aaa"

    def test_search_by_history_db_id(self):
        e1 = make_history_entry(id="h1", file="s1.zst")
        db_id = ShotDataBase.insert_history(e1)

        params = SearchParams(ids=[db_id], dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1

    def test_search_by_multiple_ids(self):
        for i in range(3):
            e = make_history_entry(id=f"uuid-{i}", file=f"s{i}.zst")
            ShotDataBase.insert_history(e)

        params = SearchParams(ids=["uuid-0", "uuid-2"], dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 2
        result_ids = {r["id"] for r in results}
        assert result_ids == {"uuid-0", "uuid-2"}


class TestDeleteShot:
    def test_delete_removes_history(self):
        entry = make_history_entry()
        db_id = ShotDataBase.insert_history(entry)

        ShotDataBase.delete_shot(db_id)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 0

    def test_delete_cleans_up_orphaned_profile(self):
        entry = make_history_entry()
        db_id = ShotDataBase.insert_history(entry)

        ShotDataBase.delete_shot(db_id)

        # Insert another shot to verify the profile was cleaned up
        p2 = make_profile(id="p-new", name="New Profile")
        e2 = make_history_entry(profile=p2, id="h2", file="s2.zst")
        ShotDataBase.insert_history(e2)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["profile"]["name"] == "New Profile"

    def test_delete_preserves_shared_profile(self):
        profile = make_profile()
        e1 = make_history_entry(profile=profile, id="h1", file="s1.zst")
        e2 = make_history_entry(profile=profile, id="h2", file="s2.zst")
        db_id1 = ShotDataBase.insert_history(e1)
        ShotDataBase.insert_history(e2)

        ShotDataBase.delete_shot(db_id1)

        params = SearchParams(dump_data=False)
        results = ShotDataBase.search_history(params)
        assert len(results) == 1
        assert results[0]["profile"]["name"] == "Test Espresso"

    def test_delete_nonexistent_shot(self):
        ShotDataBase.delete_shot(99999)
