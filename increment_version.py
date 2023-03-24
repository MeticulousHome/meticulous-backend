import re
import sys

# Read the current version number from version.py
with open("version.py", "r") as f:
    version_file = f.read()
    current_version = re.search(r'VERSION\s*=\s*[\'"]([^\'"]*)[\'"]', version_file).group(1)

# Parse the command-line argument
if len(sys.argv) < 2:
    print("Please specify major, minor, or patch as an argument")
    sys.exit(1)

if sys.argv[1] not in ["major", "minor", "patch"]:
    print(f"Invalid argument: {sys.argv[1]}. Please specify major, minor, or patch")
    sys.exit(1)

# Increment the version number
version_parts = current_version.split(".")
if sys.argv[1] == "major":
    version_parts[0] = str(int(version_parts[0]) + 1)
    version_parts[1] = "0"
    version_parts[2] = "0"
elif sys.argv[1] == "minor":
    version_parts[1] = str(int(version_parts[1]) + 1)
    version_parts[2] = "0"
else:
    version_parts[2] = str(int(version_parts[2]) + 1)

new_version = ".".join(version_parts)

# Write the new version number to version.py
with open("version.py", "w") as f:
    f.write(f'VERSION = "{new_version}"\n')

print(f"Version incremented from {current_version} to {new_version}")