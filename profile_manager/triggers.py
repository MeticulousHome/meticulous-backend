import json

#This class is used to create the triggers for the complex JSON
class Triggers:
        
        def __init__(self):
            self.data = {}
            # A dictionary to store the correspondence between the trigger type from the JSON simplified and the trigger type for the complex JSON
            self.triggers = {
                'flow': {
                    'value': 'flow_value_trigger',
                    'curve': 'flow_curve_trigger'
                },
                'piston_position': 'piston_position_trigger',
                'power': {
                    'value': 'piston_power_value_trigger',
                    'curve': 'piston_power_curve_trigger'    
                },
                'pressure': {
                    'value': 'pressure_value_trigger',
                    'curve': 'pressure_curve_trigger'
                },
                'speed': 'piston_speed_trigger',
                'temperature':{
                    'value': 'temperature_value_trigger',
                    'curve': 'temperature_curve_trigger'
                },
                'time': 'timer_trigger',
                "water_detection" : "water_detection_trigger",
                'weight': 'weight_value_trigger',
                "button" : "button_trigger",
                "exit" : "exit"
            }
            # A dictionary to store the correspondence between the source from the JSON simplified and the source for the complex JSON
            self.sources = {
                'flow': {
                    'raw': 'Flow Raw',
                    'predictive': 'Flow Predictive',
                    'average': 'Flow Average'
                },
                'pressure':{
                    'raw': 'Pressure Raw',
                    'predictive': 'Pressure Predictive',
                    'average': 'Pressure Average'
                },
                'power': {
                    'raw': 'Piston Power Raw',
                    'predictive': 'Piston Power Predictive',
                    'average': 'Piston Power Average'
                },
                'weight': {
                    'raw': 'Weight Raw',
                    'predictive': 'Weight Predictive',
                    'average': 'Weight Average'  
                },
                'temperature': {
                    'tube':'Tube Temperature', 
                    'cylinder':'Cylinder Temperature', 
                    'plunger':'Plunger Temperature', 
                    'water':'Water Temperature', 
                    'cylinder Average': 'Cylinder Temperature Average'
                },
                'button' : {
                    'start' : 'Start Button',
                    'tare' : 'Tare Button',
                    'encoder' : 'Encoder',
                    'encoder Button' : 'Encoder Button'
                    },
                'button_gesture' :{ 
                    'single': 'Single Tap',
                    'double': 'Double Tap',
                    'right': 'Right',
                    'left': 'Left',
                    'pressed': 'Pressed',
                    'released': 'Released',
                    'long': 'Long Press'  
                },
            }
            
        def flow_value_trigger(self, flow_value: float, node_id: int):
            self.data = {
                "kind": self.triggers['flow']['value'],
                "source": self.sources['flow']['raw'],
                "operator": ">=",
                "value": flow_value,
                "next_node_id": node_id
            }
            return self.data
            
        def flow_curve_trigger(self, curve_id: int, node_id: int):
            self.data = {
                "kind": self.triggers['flow']['curve'],
                "source": self.sources['flow']['raw'],
                "operator": ">=",
                "curve_id": curve_id,
                "next_node_id": node_id
            }
            return self.data
        
        def piston_position_trigger(self, position_value: float, node_id: int, position_reference: int):
            self.data = {
                "kind": self.triggers['piston_position'],
                "source": "Piston Position Raw",
                "operator": ">=",
                "value": position_value,
                "position_reference_id": position_reference,
                "next_node_id": node_id
            }
            return self.data
        
        def piston_power_value_trigger(self, power_value: float, node_id: int):
            self.data = {
                "kind": self.triggers['power']['value'],
                "source": self.sources['power']['raw'],
                "operator": ">=",
                "value": power_value,
                "next_node_id": node_id
            }
            return self.data
        
        def piston_power_curve_trigger(self, curve_id: int, node_id: int):
            self.data = {
                "kind": self.triggers['power']['curve'],
                "source": self.sources['power']['raw'],
                "operator": ">=",
                "curve_id": curve_id,
                "next_node_id": node_id
            }
            return self.data
        
        def pressure_value_trigger(self, pressure_value: float, node_id: int):
            self.data = {
                "kind": self.triggers['pressure']['value'],
                "source": self.sources['pressure']['raw'],
                "operator": ">=",
                "value": pressure_value,
                "next_node_id": node_id
            }
            return self.data
        
        def pressure_curve_trigger(self, curve_id: int, node_id: int):
            self.data = {
                "kind": self.triggers['pressure']['curve'],
                "source": self.sources['pressure']['raw'],
                "operator": ">=",
                "curve_id": curve_id,
                "next_node_id": node_id
            }
            return self.data
        
        def piston_speed_trigger(self, speed_value: float, node_id: int):
            self.data = {
                "kind": self.triggers['speed'],
                "operator": ">=",
                "value": speed_value,
                "next_node_id": node_id
            }
            return self.data
        
        def temperature_value_trigger(self, temperature_value: float, node_id: int, source: str):
            self.data = {
                "kind": self.triggers['temperature']['value'],
                "source": source,
                "operator": ">=",
                "value": temperature_value,
                "next_node_id": node_id
            }
            return self.data
            
        def temperature_curve_trigger(self, curve_id: int, node_id: int, source: str):
            self.data = {
                "kind": self.triggers['temperature']['curve'],
                "source": source,
                "operator": ">=",
                "curve_id": curve_id,
                "next_node_id": node_id
            }
            return self.data
            
        def timer_trigger(self, time_value: float, node_id: int, time_reference: int):
            self.data = {
                "kind": self.triggers['time'],
                "operator": ">=",
                "value": time_value,
                "timer_reference_id": time_reference,
                "next_node_id": node_id
            }
            return self.data
        
        def water_detection_trigger(self, node_id: int, value: bool):
            self.data = {
                "kind": self.triggers['water_detection'],
                "value": value,
                "next_node_id": node_id
            }
            return self.data
            
        def weight_value_trigger(self, weight_value: float, node_id: int, weight_reference: int):
            self.data = {
                "kind": self.triggers['weight'],
                "source": self.sources['weight']['raw'],
                "operator": ">=",
                "value": weight_value,
                "weight_reference_id": weight_reference,
                "next_node_id": node_id
            }
            return self.data
            
        def button_trigger(self, node_id: int, gesture: str, source: str):
            self.data = {
                "kind": self.triggers['button'],
                "source": source,
                "gesture": gesture,
                "next_node_id": node_id
            }
            return self.data
        
        def exit(self, node_id: int):
            self.data = {
                "kind": self.triggers['exit'],
                "next_node_id": node_id
            }
            return self.data
            
            
if __name__ == '__main__':
    
    """
    Example usage of the Triggers class.

    This class allows setting up triggers for monitoring various metrics such as flow, power, 
    pressure, and temperature. There are two main types of triggers: 'value' and 'curve'.

    - 'value' Trigger: Compares a metric against a specific numeric value. Useful for setting 
      thresholds or specific target values.
    - 'curve' Trigger: Allows comparison against a predefined graph.

    Data Sources:
    
    We have multiple sources:
    
    - Flow, Pressure, Power, and Weight: Can be derived from 'raw', 'predictive', or 'average' data.
    - Temperature: Supported sources include 'tube', 'cylinder', 'plunger', 'water', and 'cylinder average'.
    - Button Inputs: Recognizes inputs from 'start', 'tare', 'encoder', and 'encoder button'.
    
    * Note: The encoder only supports 'right' and 'left' gestures and the encoder button supports the remaining gestures.

    User Interactions:
    
    The class also supports various gestures for button interaction:
    
    - Gestures include 'single' tap, 'double' tap, swipe 'right', swipe 'left', 'pressed', 'released', and 'long' press.
    """
    Triggers_example = Triggers()
    
    print(Triggers_example.flow_value_trigger(1, 2))
    print(Triggers_example.flow_curve_trigger(1, 2))
    print(Triggers_example.piston_position_trigger(1, 2, 3))
    print(Triggers_example.piston_power_value_trigger(1, 2))
    print(Triggers_example.piston_power_curve_trigger(1, 2))
    print(Triggers_example.pressure_value_trigger(1, 2))
    print(Triggers_example.pressure_curve_trigger(1, 2))
    print(Triggers_example.piston_speed_trigger(1, 2))
    print(Triggers_example.temperature_value_trigger(1, 2, Triggers_example.sources['temperature']['water']))
    print(Triggers_example.temperature_curve_trigger(1, 2, Triggers_example.sources['temperature']['water']))
    print(Triggers_example.timer_trigger(1, 2, 3))
    print(Triggers_example.water_detection_trigger(1, True))
    print(Triggers_example.weight_value_trigger(1, 2, 3))
    print(Triggers_example.button_trigger(1, Triggers_example.sources['button_gesture']['single'], Triggers_example.sources['button']['start']))
    print(Triggers_example.exit(1))