import json
from dictionaries import reference_type


class References:
    def __init__(self):
        self.data = {}
        self.id = 0

    def generate_reference_id(self, value: int):

        return value

    def get_reference_id(self, value: int):
        return self.generate_reference_id(value)

    def set_id(self, id: int):
        self.id = self.get_reference_id(id)
        return self.id

    def set_reference_kind(self, key: str, value: str):
        self.reference = reference_type[key][value]
        return self.reference

    def get_reference_kind(self):
        return self.set_reference()

    def generate_dictionary(self, key: str, value: str, id: int):
        self.data = {"kind": self.set_reference_kind(key, value), "id": self.set_id(id)}
        return self.data


class ReferenceTime(References):
    def __init__(self):
        self.data = {}
        self.kind = "time_reference"

    def set_time_id(self):
        self.time_id = self.set_id(4)
        return self.time_id

    def get_time_dictionary(self, kind: str):
        self.id = self.set_time_id()
        self.data = self.generate_dictionary(kind, "time", self.id)
        return self.data


reference_time = ReferenceTime()
reference_time.id = 4


class ReferenceWeight(References):
    def __init__(self):
        self.data = {}

    def set_weight_id(self):
        self.weight_id = self.set_id(5)
        return self.weight_id

    def get_weight_dictionary(self, kind: str):
        self.id = self.set_weight_id()
        self.data = self.generate_dictionary(kind, "weight", self.id)
        return self.data


class ReferencePosition(References):
    def __init__(self):
        self.data = {}

    def set_position_id(self):
        self.position_id = self.set_id(6)
        return self.position_id

    def get_position_dictionary(self, kind: str):
        self.id = self.set_position_id()
        self.data = self.generate_dictionary(kind, "position", self.id)
        return self.data


if __name__ == "__main__":

    reference_time = ReferenceTime()
    weight_reference = ReferenceWeight()
    position_reference = ReferencePosition()

    print(json.dumps(reference_time.get_time_dictionary("curve"), indent=4))
    print(json.dumps(reference_time.get_time_dictionary("control"), indent=4))

    # references = References()
    # print(json.dumps(references.set_reference_kind("time"), indent=4))
    # print(json.dumps(reference_time.get_reference_kind(), indent=4))
