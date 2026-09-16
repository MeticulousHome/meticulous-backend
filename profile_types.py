def is_cleaning_profile(profile: dict) -> bool:
    return profile.get("profile_type") == "cleaning"


def is_node_profile(profile: dict) -> bool:
    stages = profile.get("stages")
    return bool(stages) and all(
        isinstance(stage, dict) and isinstance(stage.get("nodes"), list) for stage in stages
    )
