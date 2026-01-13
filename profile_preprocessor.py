import json
import os
from copy import deepcopy

from config import CONFIG_PATH
from jsonschema import ValidationError, validate
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# Custom exceptions for handling specific errors


class UndefinedVariableException(Exception):
    """Exception for when a variable is referenced but not defined."""

    pass


class VariableTypeException(Exception):
    """Exception for when a variable's type does not match the expected type."""

    pass


class FormatException(Exception):
    """Exception for when a variable's type does not match the expected type."""

    pass


SCHEMA_FILENAME = "schema.json"


class ProfilePreprocessor:

    _json_schema = None
    _json_schema_last_modified = None

    @staticmethod
    def _load_and_cache_json_schema():
        """Loads the JSON schema from disk if it's not loaded or if it has been updated since the last load."""

        schema_path = os.path.join(CONFIG_PATH, SCHEMA_FILENAME)

        # Check if the schema file has been modified since last loaded
        try:
            last_modified = os.path.getmtime(schema_path)
            if (
                ProfilePreprocessor._json_schema is None
                or last_modified > ProfilePreprocessor._json_schema_last_modified
            ):
                with open(schema_path, "r") as schema_file:
                    ProfilePreprocessor._json_schema = json.load(schema_file)
                    ProfilePreprocessor._json_schema_last_modified = last_modified
        except OSError as e:
            logger.warning(f"Error loading JSON schema: {e}")

    @staticmethod
    def validateJSON(json_data):
        ProfilePreprocessor._load_and_cache_json_schema()
        if ProfilePreprocessor._json_schema is None:
            logger.error("JSON schema couldn't be loaded. Skipping (this is bad!)")
            return

        try:
            validate(instance=json_data, schema=ProfilePreprocessor._json_schema)
        except ValidationError as e:
            raise FormatException(f"Validation error: {e.message}")

    @staticmethod
    def _replace_variable(value_or_variable, expected_type, variables_map):
        """Checks and replaces variables in points with their values, ensuring type correctness."""
        if isinstance(value_or_variable, str):
            if not value_or_variable.startswith("$"):
                raise FormatException(
                    f"Entry {value_or_variable} is not referencing a variable but is a string"
                )
            var_key = value_or_variable[1:]
            if var_key not in variables_map:
                raise UndefinedVariableException(f"Variable {var_key} is not defined")
            var_value, var_type = variables_map[var_key]
            if var_type != expected_type:
                raise VariableTypeException(
                    f"Variable {var_key} of type {var_type} used as {expected_type}"
                )
            return var_value
        return value_or_variable

    @staticmethod
    def processVariables(in_profile: dict):
        profile = deepcopy(in_profile)
        rs_multiplier: float = 10
        # Map of variables for quick lookup
        variables_map = {
            var["key"]: (var["value"], var["type"])
            for var in profile.get("variables", [])
        }

        rs_variable = variables_map.get("retraction_speed", (float(10), "errorType"))
        try:
            rs_multiplier = float(rs_variable[0])
            logger.debug(f"new retraction speed {rs_multiplier*100}")
        except Exception:
            logger.warning("failed getting retraction speed as int, setting to 10")
            rs_multiplier = 10.0

        try:
            # Iterate over stages to replace variables in points and exit triggers
            for stage_index, stage in enumerate(profile.get("stages", [])):
                # Ensure necessary keys exist in stage at least to the degree needed for variable processing
                if "type" not in stage:
                    raise FormatException(f"stage {stage_index} missing 'type' field")

                if "dynamics" not in stage:
                    raise FormatException(
                        f"stage {stage_index} missing 'dynamics' field"
                    )

                if "points" not in stage["dynamics"]:
                    raise FormatException(
                        f"stage {stage_index} dynamics are missing the points array"
                    )

                stage_type = stage["type"]

                # Process points
                for point in stage["dynamics"]["points"]:
                    # Given time is the most common "over" type as accept omitting it here
                    point_x_type = stage.get(stage["dynamics"]["over"], "time")
                    point[0] = ProfilePreprocessor._replace_variable(
                        point[0], point_x_type, variables_map
                    )
                    point[1] = ProfilePreprocessor._replace_variable(
                        point[1], stage_type, variables_map
                    )

                # Process ExitTriggers
                for trigger_index, trigger in enumerate(stage.get("exit_triggers", [])):
                    if "type" not in trigger:
                        raise FormatException(
                            f"exitTrigger {trigger_index} missing 'type' field"
                        )
                    if "value" not in trigger:
                        raise FormatException(
                            f"exitTrigger {trigger_index} missing 'value' field"
                        )

                    trigger["value"] = ProfilePreprocessor._replace_variable(
                        trigger["value"], trigger["type"], variables_map
                    )

                # Process limits
                for limit_index, limit in enumerate(stage.get("limits", [])):
                    if "type" not in limit:
                        raise FormatException(
                            f"limit {limit_index} missing 'type' field"
                        )
                    if "value" not in limit:
                        raise FormatException(
                            f"limit {limit_index} missing 'value' field"
                        )

                    limit["value"] = ProfilePreprocessor._replace_variable(
                        limit["value"], limit["type"], variables_map
                    )

        except KeyError as e:
            raise FormatException(f"Missing field detected: {e}")
        except TypeError as e:
            raise FormatException(f"Invalid format detected: {e}")

        return (profile, rs_multiplier)
