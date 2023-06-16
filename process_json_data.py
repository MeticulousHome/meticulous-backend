import json
import stages.stages as stages

def get_curve(kind,payload,init_node,end_node):
    print(init_node)
    print(end_node)

def generate_json(payload):
    init_node = 300
    end_node = init_node + 1
    stages = payload["stages"]
    #Hacemos un for para recorrer los stages
    for stage in stages:
        if stage["name"] == "Preinfusion":
            preinfusion = get_curve("Preinfusion",[stage],init_node,end_node)
            pass
        if stage["name"] == "Infusion":
            Infusion = get_curve("Preinfusion",[stage],init_node+1,end_node+1)
        if stage["name"] == "spring":
            spring = get_curve("Preinfusion",[stage],init_node+1,end_node+1)
        

if __name__ == "__main__":
    # Read the JSON file
    with open("dashboard.json", "r") as f:
        data = json.load(f)

    # Call the function with the loaded data
    generate_json(data)
