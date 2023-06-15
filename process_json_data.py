import json
from stages.stages import select_json_structure

def generate_json(payload):
    pass


if __name__ == "__main__":
    with open(".json/dashboard.json", "r") as f:
        data = json.load(f)
