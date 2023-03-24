import re
import sys
import datetime

# Read the current version number from version.py
with open("version.py", "r") as f:
    version_file = f.read()
    current_version = re.search(
        r'VERSION\s*=\s*[\'"]([^\'"]*)[\'"]', version_file).group(1)

# Split the version number into its parts
version_parts = current_version.split("-")[0].split(".")

# Parse the command-line argument, or default to "build"
if len(sys.argv) >= 2 and sys.argv[1] in ["major", "minor", "patch"]:
    part_to_increment = sys.argv[1]
    version_parts = version_parts[:3]
else:
    part_to_increment = "build"

# Increment the version number
if part_to_increment == "major":
    version_parts[0] = str(int(version_parts[0]) + 1)
    version_parts[1] = "0"
    version_parts[2] = "0"
elif part_to_increment == "minor":
    version_parts[1] = str(int(version_parts[1]) + 1)
    version_parts[2] = "0"
elif part_to_increment == "patch":
    version_parts[2] = str(int(version_parts[2]) + 1)

# Generate the new version number and write it to version.py
if part_to_increment != "build":
    new_version = ".".join(version_parts)
else:
    # Generate the build number based on the current date and a sequential number
    today = datetime.date.today().strftime("%Y%m%d")
    try:
        with open("build.txt", "r") as f:
            build_number = int(f.read().strip())
    except FileNotFoundError:
        build_number = 0
    build_number += 1
    # Update the version number with the new build number
    if len(version_parts) > 3:
        last_date = version_parts[-2]
        if last_date != today:
            build_number = 1
        version_parts = version_parts[:-2]
    version_parts.append(today)
    version_parts.append(str(build_number))
    new_version = ".".join(version_parts[:-2])
    new_version += f"-{version_parts[-2]}.{version_parts[-1]}"

    with open("build.txt", "w") as f:
        f.write(str(build_number))

with open("version.py", "w") as f:
    f.write(f'VERSION = "{new_version}"\n')

# Print a message to the console indicating the old and new version numbers
print(f"Version incremented from {current_version} to {new_version}")
