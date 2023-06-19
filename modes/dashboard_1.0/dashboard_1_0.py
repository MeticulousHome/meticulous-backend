import json
from dashboard_1_0_stages import get_stages as get_stages

def generate_dashboard_1_0(parameters: dict):
    try:
        name = parameters["name"]
    except:
        name = "Dashboard 1.0"
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

    dashboard_json = {
        "name": name,
        "stages": stages,
        "source": source,
    }

    return json.dumps(dashboard_json, indent=4)

if __name__ == "__main__":
    #import json parameters from file
    parameters = None
    with open("parameters.json", "r") as parameters_file:
        parameters = json.load(parameters_file)

    payload = generate_dashboard_1_0(parameters)

    print(payload)