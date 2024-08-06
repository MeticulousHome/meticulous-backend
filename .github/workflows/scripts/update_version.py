import sys
import re
from datetime import datetime
import json


def main(ref, pr_comments, version_bump_type):
    with open("version.py", "r") as f:
        version_file = f.read()

    current_version = re.search(
        r'VERSION\s*=\s*[\'"]([^\'"]*)[\'"]', version_file
    ).group(1)
    major, minor, patch = map(int, current_version.split("."))

    if version_bump_type == "major":
        major += 1
        minor = 0
        patch = 0
    elif version_bump_type == "minor":
        minor += 1
        patch = 0
    else:  # patch
        patch += 1

    new_version = f"{major}.{minor}.{patch}"

    version_file = re.sub(
        r'VERSION\s*=\s*["\']([^\'"]*)["\']', f'VERSION = "{new_version}"', version_file
    )

    with open("version.py", "w") as f:
        f.write(version_file)

    with open("CHANGELOG.md", "r") as f:
        changelog_contents = f.read()

    # pr_comments = json.loads(pr_comments)
    # change_description = "\n".join([f"- {c['body']}" for c in pr_comments])

    if version_bump_type == "major":
        heading_level = "#"
    elif version_bump_type == "minor":
        heading_level = "##"
    else:  # patch
        heading_level = "###"

    new_entry = (
        f"\n{heading_level} [v{new_version}] - {datetime.now().strftime('%Y-%m-%d')}\n"
        f"{pr_comments}\n"
    )

    changelog_contents = f"{new_entry}\n{changelog_contents}"

    with open("CHANGELOG.md", "w") as f:
        f.write(changelog_contents)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3])
