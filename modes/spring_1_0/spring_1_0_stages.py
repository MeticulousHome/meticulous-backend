import json
from prepurge_stage import get_prepurge_stage as get_prepurge_stage
from heating_stage import get_heating_stage as get_heating_stage
from retracting_stage import get_retracting_stage as get_retracting_stage
from closing_valve_stage import get_closing_valve_stage as get_closing_valve_stage
from preinfusion_stage import get_preinfusion_stage as get_preinfusion_stage
from idle_stage import get_idle_stage as get_idle_stage
from retracting_2_stage import get_retracting_2_stage as get_retracting_2_stage
from spring_1_0_stage import get_spring_stage as get_spring_stage
from remove_cup_stage import get_remove_cup_stage as get_remove_cup_stage
from end_purge_stage import get_end_purge_stage as get_end_purge_stage

def get_stages(parameters: json):    
    stages = []
    current_stage = 300

    prepurge_stage = get_prepurge_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    heating_stage = get_heating_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    retracting_stage = get_retracting_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    closing_valve_stage = get_closing_valve_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    if (parameters["preinfusion"]):
        preinfusion_stage = get_preinfusion_stage(parameters, current_stage, current_stage + 1)
        current_stage += 1
    spring_stage = get_spring_stage (parameters, current_stage, current_stage + 1)
    current_stage += 1
    idle_stage = get_idle_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    retracting_2_stage = get_retracting_2_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    if (parameters["automatic_purge"]):
        remove_cup_stage = get_remove_cup_stage(parameters, current_stage, current_stage + 1)
        current_stage += 1
        end_purge_stage = get_end_purge_stage(parameters, current_stage, current_stage + 1)
        current_stage += 1

    stages.append(prepurge_stage)
    stages.extend(heating_stage if isinstance(heating_stage, list) else [heating_stage])
    stages.append(retracting_stage)
    stages.append(closing_valve_stage)
    if (parameters["preinfusion"]):
        stages.append(preinfusion_stage)
    stages.append(spring_stage)
    stages.append(idle_stage)
    stages.append(retracting_2_stage)
    if (parameters["automatic_purge"]):
        stages.append(remove_cup_stage)
        stages.append(end_purge_stage)

    return stages