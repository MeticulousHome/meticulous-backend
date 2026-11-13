//! `profile_converter/triggers.py`: the triggers the manual nodes carry.
//!
//! `OperatorTriggers.__init__` seeds `kind`, `operator` and `value` before
//! the subclass adds its own keys, so those three come first in every
//! operator trigger whatever order the subclass assigns them in.

use serde_json::{Map, Value};

use super::dictionaries::{operator_type, source_type, trigger_type};

/// A built trigger dict.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger(Map<String, Value>);

impl Trigger {
    pub fn set_next_node_id(&mut self, node_id: i64) {
        self.0.insert("next_node_id".into(), Value::from(node_id));
    }

    pub fn get_trigger(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

fn entries(pairs: Vec<(&str, Value)>) -> Trigger {
    let mut data = Map::new();
    for (key, value) in pairs {
        data.insert(key.to_string(), value);
    }
    Trigger(data)
}

/// `ExitTrigger(next_node_id)`.
pub fn exit_trigger(next_node_id: i64) -> Trigger {
    entries(vec![
        ("kind", Value::from(trigger_type::EXIT)),
        ("next_node_id", Value::from(next_node_id)),
    ])
}

/// `ButtonTrigger.gesture`, as `ButtonGestureSourceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Single,
    Long,
}

/// `ButtonTrigger(ButtonSourceType.ENCODER_BUTTON, gesture, next_node_id)`.
pub fn encoder_button_trigger(gesture: Gesture, next_node_id: i64) -> Trigger {
    let gesture = match gesture {
        Gesture::Single => source_type::GESTURE_SINGLE,
        Gesture::Long => source_type::GESTURE_LONG,
    };
    entries(vec![
        ("kind", Value::from(trigger_type::BUTTON)),
        ("source", Value::from(source_type::BUTTON_ENCODER_BUTTON)),
        ("gesture", Value::from(gesture)),
        ("next_node_id", Value::from(next_node_id)),
    ])
}

/// `UserFinishTrigger(next_node_id)`: the exit the ESP fires on
/// `action,finish`. `TriggerType` has no member for it; the kind is spelled
/// out like the node runtime's other own kinds.
pub fn user_finish_trigger(next_node_id: i64) -> Trigger {
    entries(vec![
        ("kind", Value::from("user_finish_trigger")),
        ("next_node_id", Value::from(next_node_id)),
    ])
}

/// `WeightTrigger(SourceType.PREDICTIVE, TriggerOperatorType.GREATER_THAN_OR_EQUAL,
/// value, weight_reference, next_node_id)`.
pub fn predictive_weight_trigger(
    value: Value,
    weight_reference: i64,
    next_node_id: i64,
) -> Trigger {
    entries(vec![
        ("kind", Value::from(trigger_type::WEIGHT)),
        (
            "operator",
            Value::from(operator_type::GREATER_THAN_OR_EQUAL),
        ),
        ("value", value),
        ("source", Value::from(source_type::PREDICTIVE_WEIGHT)),
        ("weight_reference_id", Value::from(weight_reference)),
        ("next_node_id", Value::from(next_node_id)),
    ])
}

/// `PistonPositionTrigger(TriggerOperatorType.GREATER_THAN_OR_EQUAL, value,
/// position_reference, next_node_id)`.
pub fn piston_position_trigger(
    value: Value,
    position_reference: i64,
    next_node_id: i64,
) -> Trigger {
    entries(vec![
        ("kind", Value::from(trigger_type::PISTON_POSITION)),
        (
            "operator",
            Value::from(operator_type::GREATER_THAN_OR_EQUAL),
        ),
        ("value", value),
        ("source", Value::from("Piston Position Raw")),
        ("position_reference_id", Value::from(position_reference)),
        ("next_node_id", Value::from(next_node_id)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `test_manual_mode.py::test_the_user_finish_trigger_is_the_contract_dict`.
    #[test]
    fn user_finish_trigger_is_the_contract_dict() {
        assert_eq!(
            user_finish_trigger(7000).get_trigger(),
            json!({"kind": "user_finish_trigger", "next_node_id": 7000})
        );
        assert_eq!(
            user_finish_trigger(0).get_trigger()["next_node_id"],
            json!(0)
        );
    }

    /// `test_manual_mode.py::test_two_user_finish_triggers_do_not_share_a_dict`.
    #[test]
    fn two_user_finish_triggers_do_not_share_a_dict() {
        let first = user_finish_trigger(7000);
        let mut second = user_finish_trigger(7000);
        second.set_next_node_id(42);
        assert_eq!(first.get_trigger()["next_node_id"], json!(7000));
        assert_eq!(second.get_trigger()["next_node_id"], json!(42));
    }
}
