import json
import stages.stages as stages



def generate_json(payload):
    if payload.get("source") == "DASHBOARD":
        print("Coming from DASHBOARD")
        # Search if the value "spring" exists anywhere in the dictionary
        found = stages.find_key_value(payload, "spring")
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
