from log import MeticulousLogger
from .api import API, APIVersion

import json

import os
from pathlib import Path

from .base_handler import BaseHandler

from dataclasses import dataclass, asdict
from typing import Any

from config import (
    MeticulousConfigDict,
)

from machine import Machine

logger = MeticulousLogger.getLogger(__name__)

MANUFACTURING_CONFIG_PATH = os.getenv(
    "MANUFACTURING_CONFIG_PATH", "/meticulous-user/manufacturing"
)

# Stage skipping
SKIP_STAGE_KEY = "skip_stage"
SKIP_STAGE_DEFAULT = False

# Persistency
PERSIST_KEY = "persist"
PERSIST_DEFAULT = False

# IN_MANUFACTURING = "manufacturing"
MANUFACTURING_SETTINGS_ENDPOINT = "manufacturing_settings"

Default_manufacturing_config = {
    SKIP_STAGE_KEY: SKIP_STAGE_DEFAULT,
    PERSIST_KEY: PERSIST_DEFAULT,
}


@dataclass
class SettingOptions:
    name: str
    type: str
    value: Any


@dataclass
class DialElement:
    key: str
    label: str
    options: list[dict]


# JSON schema with the info required by the dial to render de adequate options
dial_schema: dict = {"Elements": []}

# fill the dial_schema
# Add enable_skip_stage option
dial_schema.setdefault("Elements", []).extend(
    [
        asdict(
            DialElement(
                key=f"{SKIP_STAGE_KEY}",
                label="Skip Stage",
                options=[
                    asdict(SettingOptions(name="enabled", type="boolean", value=True)),
                    asdict(
                        SettingOptions(name="disabled", type="boolean", value=False)
                    ),
                ],
            )
        ),
        asdict(
            DialElement(
                key=f"{PERSIST_KEY}",
                label="Enable Manufacturing",
                options=[
                    asdict(SettingOptions(name="enabled", type="boolean", value=True)),
                    asdict(
                        SettingOptions(name="disabled", type="boolean", value=False)
                    ),
                ],
            )
        ),
    ]
)


class MeticulousManufacturingConfigDict(MeticulousConfigDict):

    # This will help with the following:
    # 1. Only create the ManufacturingConfig file if:
    #     a) It does not exist and the Machine is in manufacturing mode
    # 2. Load the file if it exists
    # and the the Machine is not set to manufacturing mode
    def __new__(cls, path, default_dict={}):
        exist_file: bool = Path(path).exists()
        if exist_file or Machine.enable_manufacturing:
            return super().__new__(cls)
        else:
            return None

    # This will load the file
    # Once the file is loaded it will check if the persistency flag is set then:
    # a) The flag is cleared:
    #   1. Check the status of Machine.enable_manufacturing
    #      - If False: it will delete the file and object's contents
    def __init__(self, path, default_dict={}):
        self.empty: bool = False
        super().__init__(path, default_dict)
        isPersistent: bool = self.get(PERSIST_KEY)

        if isPersistent:
            return

        if Machine.enable_manufacturing:
            return

        self.empty: bool = True
        super().__init__(path, {})

    @staticmethod
    def delete_object(instance):
        if instance is not None:
            del instance


class ManufacturingSetter:

    def __init__(self):
        self.ManufacturingConfig: MeticulousManufacturingConfigDict | None = None

    def get_config_obj(self) -> MeticulousManufacturingConfigDict | None:
        return self.ManufacturingConfig

    def update_conf_obj(self) -> None:
        self.ManufacturingConfig = MeticulousManufacturingConfigDict(
            MANUFACTURING_CONFIG_PATH, Default_manufacturing_config
        )

    def delete_config(self) -> None:
        MeticulousManufacturingConfigDict.delete_object(self.ManufacturingConfig)


manufacturing_config_wrapper: ManufacturingSetter = ManufacturingSetter()


class ManufacturingSettingsHandler(BaseHandler):
    last_progress: float = 0
    last_extra_info: str = None
    __sio = None

    @classmethod
    def to_json(cls):
        extra_info_str = (
            f" : {cls.last_extra_info}"
            if cls.last_extra_info is not None and isinstance(cls.last_extra_info, str)
            else ""
        )
        return {
            "progress": round(cls.last_progress),
            "info": extra_info_str,
        }

    @classmethod
    def setSio(cls, sio):
        cls.__sio = sio

    # When the dial request data from the endpoint it will provide the schema if
    # the machine is on manufacturing mode
    def get(self, config=None):
        response: dict = {}
        if Machine.enable_manufacturing is True:
            self.set_status(203)  # Report no content
        else:
            self.set_status(200)
            response = dial_schema  # Report the schema
        self.write(response)

    # We are expecting to only handle a single setting with each request
    # the body is a json
    # {
    #   "value": <value>
    # }
    def post(self, config=None):

        if config is not None:
            current_config: MeticulousManufacturingConfigDict = (
                manufacturing_config_wrapper.get_config_obj()
            )

            current_value = current_config.get(config)
            try:
                new_value = json.loads(self.request.body).get("value")
            except json.decoder.JSONDecodeError as e:
                self.set_status(403)
                self.write(
                    {"status": "error", "error": "invalid json", "json_error": f"{e}"}
                )
                return

            if current_value is not None:
                if type(new_value) is not type(current_value):
                    self.set_status(404)
                    self.write(
                        {
                            "status": "error",
                            "error": f"setting value invalid, expected {type(current_value)}",
                            "setting": config,
                            "value": current_value,
                        }
                    )
                    current_config.load()
                    return

                # Add special cases through if statements

                manufacturing_config_wrapper.get_config_obj()[config] = new_value
            else:
                self.set_status(404)
                self.write(
                    {"status": "error", "error": "setting not found", "setting": config}
                )
        else:
            self.write({"status": "error", "error": "no setting provided"})

        manufacturing_config_wrapper.get_config_obj().save()


API.register_handler(APIVersion.V1, r"/manufacturing/(*)", ManufacturingSettingsHandler)
