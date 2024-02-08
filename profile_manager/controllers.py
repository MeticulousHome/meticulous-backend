import json 

# A dictionary to store the correspondence between the type from the JSON simplified and the controller for the complex JSON

controllers = {
    "power": "piston_power_controller",
    "flow" : "flow_controller",
    "pressure" : "pressure_controller",
    "weight" : "weight_controller",
    "speed" : "move_piston_controller",
    "temperature" : "temperature_controller",
    "tare" : "tare_controller",
    "message" : "log_controller",
    "end" : "end_profile",
    "position" : "position_reference",
    "time" : "time_reference",
    "weight" : "weight_reference"
} 