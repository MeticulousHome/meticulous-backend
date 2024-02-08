import json

class HeadProfile:
    
    def __init__(self):
        self.data = {}
        
    def click_to_start(self, click: bool):
        
        file_path = "head_click_to_start.json" if click else "head_no_click_to_start.json"
        try:
            with open(file_path, 'r') as file:
                self.data = json.load(file)
        except FileNotFoundError:
            print(f"File {file_path} not found")
            return {}
        
        

class TailProfile:
    
    def __init__(self):
        self.parameters = {}    
        
if __name__ == "__main__":
    # Example usage of the HeadProfile and TailProfile classes.
    file_path = "head_profile_example.json"
    
    head_profile = HeadProfile()
    head_profile.click_to_start(True)
    print(json.dumps(head_profile.data, indent=4))
    
    # tail_profile = TailProfile()
    # print(json.dumps(tail_profile.parameters, indent=4))