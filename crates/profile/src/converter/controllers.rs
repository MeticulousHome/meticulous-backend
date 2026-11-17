//! `profile_converter/controllers.py`: the reference controllers `InitNode`
//! holds.

use serde_json::{Map, Value};

use super::dictionaries::reference_type;

/// `TimeReferenceController`, `WeightReferenceController` and
/// `PositionReferenceController`: `{"kind": ..., "id": ...}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceController {
    kind: &'static str,
    id: i64,
}

impl ReferenceController {
    pub fn time(id: i64) -> Self {
        ReferenceController {
            kind: reference_type::TIME,
            id,
        }
    }

    pub fn weight(id: i64) -> Self {
        ReferenceController {
            kind: reference_type::WEIGHT,
            id,
        }
    }

    pub fn position(id: i64) -> Self {
        ReferenceController {
            kind: reference_type::POSITION,
            id,
        }
    }

    pub fn set_reference_id(&mut self, id: i64) {
        self.id = id;
    }

    pub fn reference_id(&self) -> i64 {
        self.id
    }

    pub fn get_controller(&self) -> Value {
        let mut data = Map::new();
        data.insert("kind".into(), Value::from(self.kind));
        data.insert("id".into(), Value::from(self.id));
        Value::Object(data)
    }
}
