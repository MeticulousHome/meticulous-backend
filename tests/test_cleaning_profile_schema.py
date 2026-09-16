import copy
import json
from pathlib import Path

import jsonschema
import pytest

from profile_types import is_cleaning_profile, is_node_profile

BACKEND_ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture(scope="module")
def profile_schema():
    with (BACKEND_ROOT / "profile_schema" / "schema.json").open() as schema_file:
        return json.load(schema_file)


@pytest.fixture
def cleaning_profile():
    with (BACKEND_ROOT / "profile_schema" / "cleaning_profile.json").open() as profile_file:
        return json.load(profile_file)


def test_cleaning_profile_is_valid(profile_schema, cleaning_profile):
    jsonschema.validate(cleaning_profile, profile_schema)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("temperature", 65.1),
        ("temperature", 0),
        ("temperature", "65"),
        ("final_weight", 0),
        ("stages", []),
    ],
)
def test_cleaning_profile_rejects_unsafe_shape(profile_schema, cleaning_profile, field, value):
    invalid_profile = copy.deepcopy(cleaning_profile)
    invalid_profile[field] = value

    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(invalid_profile, profile_schema)


def test_cleaning_profile_rejects_reordered_node_stages(profile_schema, cleaning_profile):
    invalid_profile = copy.deepcopy(cleaning_profile)
    invalid_profile["stages"][2], invalid_profile["stages"][3] = (
        invalid_profile["stages"][3],
        invalid_profile["stages"][2],
    )

    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(invalid_profile, profile_schema)


def test_cleaning_profile_rejects_unknown_node_operations(profile_schema, cleaning_profile):
    invalid_profile = copy.deepcopy(cleaning_profile)
    invalid_profile["stages"][2]["nodes"][1]["controllers"][0][
        "kind"
    ] = "raw_motor_power_controller"

    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(invalid_profile, profile_schema)


def test_existing_espresso_profile_remains_valid(profile_schema):
    with (BACKEND_ROOT / "profile_schema" / "example_profile.json").open() as profile_file:
        espresso_profile = json.load(profile_file)

    jsonschema.validate(espresso_profile, profile_schema)

    espresso_profile["profile_type"] = "espresso"
    jsonschema.validate(espresso_profile, profile_schema)


def test_unknown_profile_type_is_rejected(profile_schema, cleaning_profile):
    cleaning_profile["profile_type"] = "maintenance"

    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(cleaning_profile, profile_schema)


def test_cleaning_uses_the_existing_node_profile_path(cleaning_profile):
    assert "final_weight" not in cleaning_profile
    assert "workflow" not in cleaning_profile
    assert is_node_profile(cleaning_profile)


def test_cleaning_is_not_treated_as_a_last_coffee_profile(cleaning_profile):
    assert is_cleaning_profile(cleaning_profile)
    assert not is_cleaning_profile({"profile_type": "espresso"})
    assert not is_cleaning_profile({})
