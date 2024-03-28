import json
from .italian_1_0_stages import get_stages as get_stages
import uuid

def generate_italian_1_0(parameters: json):
    try:
        name = parameters["name"]
    except:
        name = "Italian 1.0"
        print("Warning: name is not defined")
    try:
        source = parameters["source"]
    except:
        source = "unknown"
        print("Warning: source is not defined")

    stages = get_stages(parameters)

    if stages is None:
        print("Error: stages is not defined")
        return None

    italian_json = {
        "name": name,
        "stages": stages,
        "source": source,
    }

    return json.dumps(italian_json, indent=4)

def convert_italian_json(parameters:json):
    # Genera un ID único
    unique_id = str(uuid.uuid4())
    output_json = {
        "name": "Simple Italian Profile",
        "id": unique_id,
        "kind": "italian_1_0",
        "temperature": parameters["temperature"] - 11,  # Ajuste de temperatura sugerido
        "final_weight": parameters["out_weight"] * 3.6,  # Ajusta según especificaciones
        "stages": []
    }
    # Si preinfusion es True, agregar el stage de Preinfusion
    if parameters["preinfusion"]:
        output_json["stages"].append({
            "name": "Preinfusion",
            "type": "flow",
            "dynamics": {
                "points": [[0, 4]],
                "over": "time",
                "interpolation": "linear"
            },
            "exit_triggers": [
                {"type": "time", "value": 30, "relative": True, "comparison": "greater"},
                {"type": "weight", "value": 0.3, "relative": True, "comparison": "greater"},
                {"type": "pressure", "value": parameters["pressure"], "relative": False, "comparison": "greater"}
            ],
            "limits": []
        })
    # Agregar el stage de Infusion siempre
    output_json["stages"].append({
        "name": "Infusion",
        "type": "pressure",
        "dynamics": {
            "points": [[0, parameters["pressure"]]],
            "over": "time",
            "interpolation": "linear"
        },
        "exit_triggers": [],
        "limits": []
    })
    return output_json


if __name__ == "__main__":
    #import json parameters from file
    parameters = None
    with open("parameters.json", "r") as parameters_file:
        parameters = json.load(parameters_file)

    payload = convert_italian_json(parameters)

    print(payload)