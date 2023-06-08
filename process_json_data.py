import json



def get_values_lever():
    with open('lever.json') as f:
        lever = json.load(f)
    # Now you can assign each field to a variable
    lever_name = lever['name']
    lever_temperature = lever['temperature']
    lever_preheat = lever['preheat']
    lever_source = lever['source']
    lever_stages = lever['stages']
    all_stages = {}
    for stage in lever_stages:
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
    head_json = {
        "name": lever_name,
        "temperature": lever_temperature,
        "preheat": lever_preheat,
        "source": lever_source,
        "stages": lever_stages
    }
    main_variables =   {**head_json, **all_stages}
    # infusion_points = combined_dict['Infusion']['points'] <-- Acees to the points of the infusion stage
    # print(infusion_points)

    return main_variables   


if __name__ == "__main__":
    example = get_values_lever()
    print(example)