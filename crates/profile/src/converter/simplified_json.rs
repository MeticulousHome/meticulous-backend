//! `profile_converter/simplified_json.py`: the profile accessors, the id
//! allocators and `InitNode`.

use serde_json::Value;

use super::controllers::ReferenceController;
use super::nodes::Nodes;
use super::triggers::{Trigger, exit_trigger};
use crate::pyops::{PyExc, getitem_str};

/// The module global `current_reference_id`: the next curve reference id.
///
/// PARITY: Python never resets it. It starts at 100 when the backend
/// starts and every manual-mode program takes three ids per stage, so the
/// reference ids (and with them the bytes and the hash sent to the ESP32)
/// differ from one load to the next within a process. The backend keeps one
/// `ReferenceIds` for the life of the process and passes it to every build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceIds {
    next: i64,
}

impl Default for ReferenceIds {
    /// The value at import: `current_reference_id = 100`.
    fn default() -> Self {
        ReferenceIds { next: 100 }
    }
}

impl ReferenceIds {
    pub fn starting_at(next: i64) -> Self {
        ReferenceIds { next }
    }

    /// The id the next allocation returns.
    pub fn peek(&self) -> i64 {
        self.next
    }

    /// `get_new_reference_id()`.
    pub fn allocate(&mut self) -> i64 {
        self.next += 1;
        self.next - 1
    }
}

/// The module global `current_node_id`, which `manual_stages` resets to
/// `END_NODE_HEAD` before every build, so it is local to one build here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeIds {
    next: i64,
}

impl NodeIds {
    pub fn starting_at(next: i64) -> Self {
        NodeIds { next }
    }

    /// `get_new_node_id()`.
    pub fn allocate(&mut self) -> i64 {
        self.next += 1;
        self.next - 1
    }
}

/// `SimplifiedJson(parameters)`: read access to the profile document.
#[derive(Debug, Clone, Copy)]
pub struct SimplifiedJson<'p> {
    parameters: &'p Value,
}

impl<'p> SimplifiedJson<'p> {
    pub fn new(parameters: &'p Value) -> Self {
        SimplifiedJson { parameters }
    }

    /// `parameters["temperature"]`.
    pub fn get_temperature(&self) -> Result<&'p Value, PyExc> {
        getitem_str(self.parameters, "temperature")
    }

    /// `parameters["name"]`.
    pub fn get_name(&self) -> Result<&'p Value, PyExc> {
        getitem_str(self.parameters, "name")
    }

    /// `parameters["final_weight"]`.
    pub fn get_final_weight(&self) -> Result<&'p Value, PyExc> {
        getitem_str(self.parameters, "final_weight")
    }
}

/// `InitNode(id)`: a node that takes time, weight and position references
/// and exits to the next node.
#[derive(Debug, Clone, PartialEq)]
pub struct InitNode {
    node: Nodes,
    time_reference: ReferenceController,
    weight_reference: ReferenceController,
    position_reference: ReferenceController,
    exit_trigger: Trigger,
}

impl InitNode {
    /// `InitNode(id)` with the constructor's defaults: references 1, 2, 3
    /// and an exit to -1.
    pub fn new(id: i64) -> Self {
        let mut node = Nodes::new(0);
        node.set_id(id);
        InitNode {
            node,
            time_reference: ReferenceController::time(1),
            weight_reference: ReferenceController::weight(2),
            position_reference: ReferenceController::position(3),
            exit_trigger: exit_trigger(-1),
        }
    }

    pub fn set_time_id(&mut self, id: i64) {
        self.time_reference.set_reference_id(id);
    }

    pub fn set_weight_id(&mut self, id: i64) {
        self.weight_reference.set_reference_id(id);
    }

    pub fn set_position_id(&mut self, id: i64) {
        self.position_reference.set_reference_id(id);
    }

    pub fn get_time_id(&self) -> i64 {
        self.time_reference.reference_id()
    }

    pub fn get_weight_id(&self) -> i64 {
        self.weight_reference.reference_id()
    }

    pub fn get_position_id(&self) -> i64 {
        self.position_reference.reference_id()
    }

    pub fn set_next_node_id(&mut self, id: i64) {
        self.exit_trigger.set_next_node_id(id);
    }

    /// The node with its time, weight and position references (in that
    /// order) and its exit trigger.
    pub fn get_node(&self) -> Value {
        let mut node = self.node.clone();
        node.add_controller(self.time_reference.get_controller());
        node.add_controller(self.weight_reference.get_controller());
        node.add_controller(self.position_reference.get_controller());
        node.add_trigger(&self.exit_trigger);
        node.get_node()
    }
}
