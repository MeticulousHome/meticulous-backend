import json
import stages.stages as stages

def get_curve(payload):
    print(payload)

def generate_json(payload):
    stages = payload["stages"]
    #Hacemos un for para recorrer los stages
    for stage in stages:
        if stage["name"] == "Preinfusion":
            preinfusion = get_curve([stage])
            pass
        if stage["name"] == "Infusion":
            Infusion = get_curve([stage])
        if stage["name"] == "spring":
            spring = get_curve([stage])
        

if __name__ == "__main__":
    # Read the JSON file
    with open("dashboard.json", "r") as f:
        data = json.load(f)

    # Call the function with the loaded data
    generate_json(data)
