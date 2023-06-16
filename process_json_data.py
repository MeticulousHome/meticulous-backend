import json
from stages.stages import select_json_structure

def find_key_value(data, target_value):
    # Traverse the dictionary and search for the target value
    if isinstance(data, dict):
        for key, value in data.items():
            if find_key_value(value, target_value):
                return True
            if key == target_value or value == target_value:
                return True

    # If it's a list, search in each element of the list
    if isinstance(data, list):
        for item in data:
            if find_key_value(item, target_value):
                return True

    # The searched value was not found
    return False

def generate_json(payload):
    if payload.get("source") == "DASHBOARD":
        print("Coming from DASHBOARD")
        # Search if the value "spring" exists anywhere in the dictionary
        found = find_key_value(payload, "spring")
        if found:
            print("The value 'spring' was found")
        else:
            print("The value 'spring' was not found")
        
    # You can add more logic here if you need to process other sources besides "DASHBOARD"

if __name__ == "__main__":
    # Read the JSON file
    with open("./json/dashboard_1_0.json", "r") as f:
        data = json.load(f)

    # Call the function with the loaded data
    generate_json(data)
