import json
from .prepurge_stage import get_prepurge_stage as get_prepurge_stage
from .heating_stage import get_heating_stage as get_heating_stage
from .retracting_stage import get_retracting_stage as get_retracting_stage
from .closing_valve_stage import get_closing_valve_stage as get_closing_valve_stage
from .idle_stage import get_idle_stage as get_idle_stage
from .retracting_2_stage import get_retracting_2_stage as get_retracting_2_stage
from .curve_stages import get_curve_stage as get_curve_stage

def get_stages(parameters: json):    
    stages = []
    current_stage = 300
    initial_node = -1
    final_node = -2

    prepurge_stage = get_prepurge_stage(parameters, initial_node, current_stage)
    heating_stage = get_heating_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    retracting_stage = get_retracting_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    closing_valve_stage = get_closing_valve_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    curve_stages = get_curve_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    idle_stage = get_idle_stage(parameters, current_stage, current_stage + 1)
    current_stage += 1
    retracting_2_stage = get_retracting_2_stage(parameters, current_stage, final_node)
    current_stage += 1

    stages.append(prepurge_stage)
    stages.extend(heating_stage if isinstance(heating_stage, list) else [heating_stage])
    stages.append(retracting_stage)
    stages.append(closing_valve_stage)
    stages.extend(curve_stages if isinstance(curve_stages, list) else [curve_stages])
    stages.append(idle_stage)
    stages.append(retracting_2_stage)

    return stages