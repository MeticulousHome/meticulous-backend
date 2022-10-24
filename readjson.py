import json
  
# Opening JSON file
f = open ('fika.json', "r")
# JSON file
  
# Reading from file
data = json.loads(f.read())
  
# Iterating through the json
# list
for i in data["stages"]:
    print(i)
  
# Closing file
f.close()