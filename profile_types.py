def is_cleaning_profile(profile: dict) -> bool:
    return profile.get("profile_type") == "cleaning"
