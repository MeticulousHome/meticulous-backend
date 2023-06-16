import os
import json

def select_json_structure(option):
    try:
        dir_path = os.path.dirname(os.path.realpath(__file__))
        file_path = os.path.join(dir_path, option + ".json")
        
        with open(file_path, 'r') as file:
            data = json.load(file)
            return data
    
    except FileNotFoundError:
        print(f"The file '{option}' does not exist.")
    
    except json.JSONDecodeError:
        print(f"The file '{option}' is not a valid JSON.")

def find_key_value(data, target_value):
    if isinstance(data, dict):
        for key, value in data.items():
            if find_key_value(value, target_value):
                return True
            
            if key == target_value or value == target_value:
                return True

    if isinstance(data, list):
        for item in data:
            if find_key_value(item, target_value):
                return True

    return False

if __name__ == "__main__":
    print(select_json_structure("retract"))
