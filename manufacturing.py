from dataclasses import asdict, dataclass
from typing import Any

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

CONFIG_MANUFACTURING = "manufacturing"


# Stage skipping
SKIP_STAGE_KEY = "skip_stage"
SKIP_STAGE_DEFAULT = False

# Persistency
FORCE_MANUFACTURING_ENABLED_KEY = "enabled"
FORCE_MANUFACTURING_ENABLED_DEFAULT = False

# Detect first normal boot
LAST_BOOT_MODE_KEY = "last_boot_mode"
LAST_BOOT_MODE_DEFAULT = None

# IN_MANUFACTURING = "manufacturing"
MANUFACTURING_SETTINGS_ENDPOINT = "manufacturing_settings"

Default_manufacturing_config = {
    FORCE_MANUFACTURING_ENABLED_KEY: FORCE_MANUFACTURING_ENABLED_DEFAULT,
    LAST_BOOT_MODE_KEY: LAST_BOOT_MODE_DEFAULT,
    SKIP_STAGE_KEY: SKIP_STAGE_DEFAULT,
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
dial_schema: dict = {
    "Elements": [
        asdict(
            DialElement(
                key=f"{SKIP_STAGE_KEY}",
                label="Skip Stage",
                options=[
                    asdict(SettingOptions(name="enabled", type="boolean", value=True)),
                    asdict(SettingOptions(name="disabled", type="boolean", value=False)),
                ],
            )
        ),
        asdict(
            DialElement(
                key=f"{FORCE_MANUFACTURING_ENABLED_KEY}",
                label="Enable Manufacturing",
                options=[
                    asdict(SettingOptions(name="enabled", type="boolean", value=True)),
                    asdict(SettingOptions(name="disabled", type="boolean", value=False)),
                ],
            )
        ),
    ]
}
