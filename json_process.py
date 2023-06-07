import json

# Assuming the content is in a file named 'data.json'
with open('data.json') as f:
    data = json.load(f)

stages = data['stages']

# Create a dictionary to store all stages
all_stages = {}

# Iterate over the stages
for stage in stages:
    stage_dict = {
        'name': stage['name'],
        'points': stage['points'],
        'stop_weight': stage['stop_weight'],
        'stop_time': stage['stop_time'],
        'max_limit_trigger': stage['max_limit_trigger'],
        'control_method': stage['control_method'],
        'interpolation_method': stage['interpolation_method']
    }
    all_stages[stage['name']] = stage_dict

# Now you can use the all_stages dictionary
preinfusion = all_stages['Preinfusion']
infusion = all_stages['Infusion']

print(preinfusion)
print(infusion)
