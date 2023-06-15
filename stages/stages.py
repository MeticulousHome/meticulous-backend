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
        print(f"El archivo '{option}' no existe.")
    except json.JSONDecodeError:
        print(f"El archivo '{option}' no es un JSON válido.")

if __name__ == "__main__":
    print(select_json_structure("retract"))
