import json

class IDGenerator:
    def __init__(self, start_value):
        self.current_value = start_value

    def get_next(self):
        self.current_value += 1
        return self.current_value


node_id_generator = IDGenerator(5000)
position_id_generator = IDGenerator(5100)
time_id_generator = IDGenerator(5200)
curve_id_generator = IDGenerator(5300)


def get_spring(start_node, end_node, max_power, min_power, pressure_limit, spring_stop_weight):
    # Assigning start and end node IDs
    INIT_NODE_ID = start_node
    END_NODE_ID = end_node

    MAX_POWER = max_power
    MIN_POWER = min_power

    PRESSURE_LIMIT = pressure_limit
    SPRING_STOP_WEIGHT = spring_stop_weight

    # Assigning consecutive IDs
    INTERNAL_NODE_1_ID = node_id_generator.get_next()
    INTERNAL_NODE_2_ID = node_id_generator.get_next()
    
    _POSITION_REFERENCE_1_ID = position_id_generator.get_next()
    _TIME_REFERENCE_1_ID = time_id_generator.get_next()
    
    _CURVE_1_ID = curve_id_generator.get_next()
    _CURVE_2_ID = curve_id_generator.get_next()

    MAX_PISTON_POSITION = 60

    template = {
        "name": "spring",
        "nodes": [
            {
                "id": INIT_NODE_ID,
                "controllers": [
                    {
                        "kind": "position_reference",
                        "id": _POSITION_REFERENCE_1_ID,
                    },
                    {
                        "kind": "time_reference",
                        "id": _TIME_REFERENCE_1_ID,
                    },
                ],
                "triggers": [
                    {
                        "kind": "exit",
                        "next_node_id": INTERNAL_NODE_1_ID,
                    },
                ],
            },
            {
                "id": INTERNAL_NODE_1_ID,
                "controllers": [
                    {
                        "kind": "piston_power_controller",
                        "algorithm": "Spring v1.0",
                        "curve": {
                            "id": _CURVE_1_ID,
                            "interpolation_kind": "catmull_interpolation",
                            "points": [[0,MAX_POWER],[MAX_PISTON_POSITION, MIN_POWER]],
                            "reference": {
                                "kind": "position",
                                "id": _POSITION_REFERENCE_1_ID,
                            },
                        },
                    },
                ],
                "triggers": [
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": ">=",
                        "value": MAX_PISTON_POSITION,
                        "next_node_id": END_NODE_ID,
                        "source": "Piston Position Raw",
                    },
                    {
                        "kind": "pressure_value_trigger",
                        "source": "Pressure Predictive",
                        "operator": ">=",
                        "value": PRESSURE_LIMIT,
                        "next_node_id": INTERNAL_NODE_2_ID,
                    },
                    {
                        "kind": "weight_value_trigger",
                        "source": "Weight Raw",
                        "weight_reference_id": 1,
                        "operator": ">=",
                        "value": SPRING_STOP_WEIGHT,
                        "next_node_id": END_NODE_ID,
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": END_NODE_ID,
                    },
                ],
            },
            {
                "id": INTERNAL_NODE_2_ID,
                "controllers": [
                    {
                        "kind": "pressure_controller",
                        "algorithm": "Pressure PID v1.0",
                        "curve": {
                            "id": _CURVE_2_ID,
                            "interpolation_kind": "linear_interpolation",
                            "points": [[0, PRESSURE_LIMIT]],
                            "reference": {
                                "kind": "time",
                                "id": _TIME_REFERENCE_1_ID,
                            },
                        },
                    },
                ],
                "triggers": [
                    {
                        "kind": "piston_power_curve_trigger",
                        "source": "Raw Piston Power",
                        "operator": ">=",
                        "curve_id": _CURVE_1_ID,
                        "next_node_id": INTERNAL_NODE_1_ID,
                    },
                    {
                        "kind": "piston_position_trigger",
                        "position_reference_id": 0,
                        "operator": ">=",
                        "value": MAX_PISTON_POSITION,
                        "next_node_id": END_NODE_ID,
                        "source": "Piston Position Raw",
                    },
                    {
                        "kind": "weight_value_trigger",
                        "source": "Weight Raw",
                        "weight_reference_id": 1,
                        "operator": ">=",
                        "value": SPRING_STOP_WEIGHT,
                        "next_node_id": END_NODE_ID,
                    },
                    {
                        "kind": "button_trigger",
                        "source": "Encoder Button",
                        "gesture": "Single Tap",
                        "next_node_id": END_NODE_ID,
                    },
                ],
            },
        ],
    }

    return template

if __name__ == "__main__":
    # Mock numbers for the function call
    start_node_mock = 100
    end_node_mock = 101
    max_power_mock = 50
    min_power_mock = 10
    pressure_limit_mock = 75
    spring_stop_weight_mock = 35

    spring_stage = get_spring(
        start_node=start_node_mock,
        end_node=end_node_mock,
        max_power=max_power_mock,
        min_power=min_power_mock,
        pressure_limit=pressure_limit_mock,
        spring_stop_weight=spring_stop_weight_mock
    )

    print("SPRING STAGE 1")
    print(json.dumps(spring_stage, indent=4))

    start_node_mock = 101
    end_node_mock = 102
    max_power_mock = 51
    min_power_mock = 11
    pressure_limit_mock = 76
    spring_stop_weight_mock = 36

    spring_stage = get_spring(
        start_node=start_node_mock,
        end_node=end_node_mock,
        max_power=max_power_mock,
        min_power=min_power_mock,
        pressure_limit=pressure_limit_mock,
        spring_stop_weight=spring_stop_weight_mock
    )

    print("SPRING STAGE 2")
    print(json.dumps(spring_stage, indent=4))
