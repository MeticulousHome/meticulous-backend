import json
import stages.stages as stages

# def get_stage(kind,stage_parameters,init_node,end_node):
#     if kind == "curve_1.0":
#         stage = get_json_curve(stage_parameters,init_node,end_node)
        
#     elif kind == "spring_1.0":
#         stage = get_json_spring(stage_parameters,init_node,end_node)
#         current_node = current_node + 1

#     return stage

def stage_modifier(kind,payload,init_node,end_node):
    json_stage_modified = stages.select_json_structure("curve")
    for key in payload:
        json_stage_modified[key] = payload[key]
    return json_stage_modified

def get_modified_stages(payload):
    current_node = 300
    stages = payload["stages"]
    json_stages = []
    for stage in stages:
        json_stages.append(stage_modifier(stage["kind"],stage,current_node,current_node+1))
        current_node = current_node + 1

    return json_stages

        

        

if __name__ == "__main__":
    # Read the JSON file
    with open("dashboard.json", "r") as f:
        data = json.load(f)
    
    print(get_modified_stages(data))
    
