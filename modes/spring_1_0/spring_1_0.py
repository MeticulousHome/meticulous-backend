import json
from spring_1_0_stages import get_stages as get_stages

def generate_spring_1_0(parameters: json):
    try:
        name = parameters["name"]
    except:
        name = "Spring 1.0"
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

    spring_json = {
        "name": name,
        "stages": stages,
        "source": source,
    }

    return json.dumps(spring_json, indent=4)

if __name__ == "__main__":
    #import json parameters from file
    parameters = None
    with open("parameters.json", "r") as parameters_file:
        parameters = json.load(parameters_file)

    payload = generate_spring_1_0(parameters)

    print(payload)