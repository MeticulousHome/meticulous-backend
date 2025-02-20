from dataclasses import dataclass, asdict
from typing import Any


CONFIG_MANUFACTURING = "manufacturing"


# Stage skipping
SKIP_STAGE_KEY = "skip_stage"
SKIP_STAGE_DEFAULT = False

# Persistency
MANUFACTURING_ENABLED_KEY = "enabled"
MANUFACTURING_ENABLED_DEFAULT = False

# Detect first normal boot
FIRST_NORMAL_BOOT_KEY = "first_normal_boot"
FIRST_NORMAL_BOOT_DEFAULT = False

# IN_MANUFACTURING = "manufacturing"
MANUFACTURING_SETTINGS_ENDPOINT = "manufacturing_settings"

Default_manufacturing_config = {
    MANUFACTURING_ENABLED_KEY: MANUFACTURING_ENABLED_DEFAULT,
    FIRST_NORMAL_BOOT_KEY: FIRST_NORMAL_BOOT_DEFAULT,
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
                    asdict(
                        SettingOptions(name="disabled", type="boolean", value=False)
                    ),
                ],
            )
        ),
        asdict(
            DialElement(
                key=f"{MANUFACTURING_ENABLED_KEY}",
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
}
