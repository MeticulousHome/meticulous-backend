import unittest
import json

from profile_preprocessor import (
    ProfilePreprocessor,
    FormatException,
    UndefinedVariableException,
    VariableTypeException,
)
import logging

logger = logging.getLogger()


class TestProfilePreprocessor(unittest.TestCase):
    def setUp(self):
        self.profile = {
            "variables": [
                {
                    "name": "Pressure",
                    "key": "pressure_1",
                    "type": "pressure",
                    "value": 9,
                },
                {"name": "Time", "key": "time_1", "type": "time", "value": 30},
            ],
            "stages": [
                {
                    "name": "Test Stage",
                    "type": "pressure",
                    "dynamics": {
                        "points": [["$time_1", "$pressure_1"]],
                        "over": "time",
                        "interpolation": "linear",
                    },
                    "exit_triggers": [{"type": "time", "value": "$time_1"}],
                    "limits": [
                        {"type": "flow", "value": 2.1},
                        {"type": "pressure", "value": "$pressure_1"},
                    ],
                }
            ],
        }

    def test_successful_processing(self):
        processed_profile = ProfilePreprocessor.processVariables(self.profile)

        self.assertEqual(
            processed_profile["stages"][0]["dynamics"]["points"][0],
            [30, 9],
            "Points were not processed correctly",
        )
        self.assertEqual(
            processed_profile["stages"][0]["exit_triggers"][0]["value"],
            30,
            "Exit were triggers not processed correctly",
        )
        self.assertEqual(
            processed_profile["stages"][0]["limits"][1]["value"],
            9,
            "Limits were not processed correctly",
        )

        logger.warning(json.dumps(processed_profile, indent=2))

    def test_missing_variables(self):
        # Remove variables
        self.profile["variables"] = []
        with self.assertRaises(UndefinedVariableException):
            ProfilePreprocessor.processVariables(self.profile)

    def test_missing_variables_field(self):
        # Remove variables
        del self.profile["variables"]
        with self.assertRaises(UndefinedVariableException):
            ProfilePreprocessor.processVariables(self.profile)

    def test_incorrect_variable_key(self):
        # Non-existent variable
        self.profile["stages"][0]["dynamics"]["points"][0][0] = "$time_2"
        with self.assertRaises(UndefinedVariableException):
            ProfilePreprocessor.processVariables(self.profile)

    def test_wrong_variable_type(self):
        # Change expected type
        self.profile["variables"][0]["type"] = "time"
        with self.assertRaises(VariableTypeException):
            ProfilePreprocessor.processVariables(self.profile)

    def test_missing_stage_key(self):
        # Remove required key
        del self.profile["stages"][0]["type"]
        with self.assertRaises(FormatException):
            ProfilePreprocessor.processVariables(self.profile)

    def test_invalid_stage_structure(self):
        # Invalid structure of the profile (array expected)
        self.profile["stages"][0]["dynamics"]["points"] = 1234
        with self.assertRaises(FormatException):
            ProfilePreprocessor.processVariables(self.profile)
