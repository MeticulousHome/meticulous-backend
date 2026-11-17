//! `profile_converter/nodes.py`.

use serde_json::{Map, Value};

use super::triggers::Trigger;

/// `Nodes(id)`: `{"id": ..., "controllers": [...], "triggers": [...]}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Nodes {
    id: i64,
    controllers: Vec<Value>,
    triggers: Vec<Value>,
}

impl Nodes {
    pub fn new(id: i64) -> Self {
        Nodes {
            id,
            controllers: Vec::new(),
            triggers: Vec::new(),
        }
    }

    pub fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    /// `add_controller(controller)`: appends `controller.get_controller()`.
    pub fn add_controller(&mut self, controller: Value) {
        self.controllers.push(controller);
    }

    /// `add_trigger(trigger)`: appends `trigger.get_trigger()`.
    pub fn add_trigger(&mut self, trigger: &Trigger) {
        self.triggers.push(trigger.get_trigger());
    }

    pub fn get_node_id(&self) -> i64 {
        self.id
    }

    pub fn get_node(&self) -> Value {
        let mut data = Map::new();
        data.insert("id".into(), Value::from(self.id));
        data.insert("controllers".into(), Value::Array(self.controllers.clone()));
        data.insert("triggers".into(), Value::Array(self.triggers.clone()));
        Value::Object(data)
    }
}
