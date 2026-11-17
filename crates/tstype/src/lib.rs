//! TypeScript types of the JSON values the backend sends.
//!
//! The Socket.IO payloads are built as `serde_json::Value`s by hand, to keep
//! the Python's key order and number types (`0` and `0.0`, the string
//! `"NaN"`), so no Rust type describes them for a derive to read. Instead each
//! builder has a [`Ts`] next to it:
//!
//! - [`check`] holds one built value against it: no undeclared key, every
//!   required key present, every value of its declared type;
//! - [`Coverage`] does that for a set of values and lists the parts of the
//!   type none of them exercised (a union member, an optional key present or
//!   absent), so a type cannot promise more than its builder sends;
//! - [`declarations`] writes the named types as TypeScript.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

/// A TypeScript type, restricted to what JSON carries.
#[derive(Debug, Clone, PartialEq)]
pub enum Ts {
    /// Any JSON number: TypeScript does not tell `1` from `1.0`.
    Number,
    String,
    Boolean,
    Null,
    /// A string literal type.
    StrLit(String),
    /// An integer literal type.
    IntLit(i64),
    Array(Box<Ts>),
    Object(Object),
    /// Members are tried in order; the first that accepts a value is the
    /// one the value exercises.
    Union(Vec<Ts>),
    /// A string holding the JSON text of a value of the inner type, where
    /// the Python emits `json.dumps(...)`. TypeScript sees a `string`.
    JsonText(Box<Ts>),
    /// A type exported under its own name.
    Named(Box<Named>),
    /// Any JSON value: what the backend relays as a client sent it.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub fields: Vec<Field>,
    /// Keys beyond `fields` and the type of their values: an index
    /// signature, `[<name>: string]: ...`.
    pub rest: Option<Box<Field>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Ts,
    /// The key may be missing (`name?: T`). A key that is always there but
    /// may hold `null` is a nullable type instead.
    pub optional: bool,
    pub doc: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub name: String,
    pub doc: String,
    pub ty: Ts,
}

impl Ts {
    pub fn literal(value: impl Into<String>) -> Ts {
        Ts::StrLit(value.into())
    }

    /// The union of these string literals.
    pub fn literals<S: Into<String>>(values: impl IntoIterator<Item = S>) -> Ts {
        Ts::union(values.into_iter().map(Ts::literal))
    }

    /// A union, with nested unions flattened.
    pub fn union(members: impl IntoIterator<Item = Ts>) -> Ts {
        let mut flat = Vec::new();
        for member in members {
            match member {
                Ts::Union(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        if flat.len() == 1 {
            flat.remove(0)
        } else {
            Ts::Union(flat)
        }
    }

    pub fn or(self, other: Ts) -> Ts {
        Ts::union([self, other])
    }

    pub fn nullable(self) -> Ts {
        self.or(Ts::Null)
    }

    pub fn array(self) -> Ts {
        Ts::Array(Box::new(self))
    }

    pub fn json_text(self) -> Ts {
        Ts::JsonText(Box::new(self))
    }

    pub fn object(fields: impl IntoIterator<Item = Field>) -> Ts {
        Ts::Object(Object {
            fields: fields.into_iter().collect(),
            rest: None,
        })
    }

    /// An object whose other keys hold `rest.ty`; `rest.name` names the key
    /// in the index signature.
    pub fn object_with_rest(fields: impl IntoIterator<Item = Field>, rest: Field) -> Ts {
        Ts::Object(Object {
            fields: fields.into_iter().collect(),
            rest: Some(Box::new(rest)),
        })
    }

    pub fn named(name: impl Into<String>, doc: impl Into<String>, ty: Ts) -> Ts {
        Ts::Named(Box::new(Named {
            name: name.into(),
            doc: doc.into(),
            ty,
        }))
    }

    /// The name of a named type.
    pub fn name(&self) -> Option<&str> {
        match self {
            Ts::Named(named) => Some(&named.name),
            _ => None,
        }
    }
}

/// A required key.
pub fn field(name: impl Into<String>, ty: Ts) -> Field {
    Field {
        name: name.into(),
        ty,
        optional: false,
        doc: String::new(),
    }
}

impl Field {
    pub fn optional(mut self) -> Field {
        self.optional = true;
        self
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Field {
        self.doc = doc.into();
        self
    }
}

/// Where a value and its type disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub path: String,
    pub expected: String,
    pub found: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: expected {}, found {}",
            self.path, self.expected, self.found
        )
    }
}

impl std::error::Error for Mismatch {}

/// Check `value` against `ty`; `root` starts the paths in the error.
pub fn check(root: &str, value: &Value, ty: &Ts) -> Result<(), Mismatch> {
    walk(value, ty, root, &mut BTreeSet::new())
}

/// [`check`] over a set of values, remembering which parts of the type they
/// exercised.
#[derive(Debug)]
pub struct Coverage<'a> {
    root: String,
    ty: &'a Ts,
    seen: BTreeSet<String>,
    values: usize,
}

impl<'a> Coverage<'a> {
    pub fn new(root: impl Into<String>, ty: &'a Ts) -> Coverage<'a> {
        Coverage {
            root: root.into(),
            ty,
            seen: BTreeSet::new(),
            values: 0,
        }
    }

    pub fn check(&mut self, value: &Value) -> Result<(), Mismatch> {
        walk(value, self.ty, &self.root, &mut self.seen)?;
        self.values += 1;
        Ok(())
    }

    /// How many values passed.
    pub fn values(&self) -> usize {
        self.values
    }

    /// The union members, optional keys (present or absent) and index
    /// signatures no checked value exercised.
    pub fn missing(&self) -> Vec<String> {
        let mut all = BTreeSet::new();
        facts(self.ty, &self.root, &mut all);
        all.difference(&self.seen).cloned().collect()
    }
}

fn child(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

fn rest_path(path: &str, rest: &Field) -> String {
    format!("{path}[{}]", rest.name)
}

fn member_fact(path: &str, member: &Ts) -> String {
    format!("{path}: {}", inline(member))
}

fn presence_fact(path: &str, present: bool) -> String {
    format!("{path} {}", if present { "present" } else { "absent" })
}

fn found(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 80 {
        format!("{}...", text.chars().take(77).collect::<String>())
    } else {
        text
    }
}

fn walk(value: &Value, ty: &Ts, path: &str, seen: &mut BTreeSet<String>) -> Result<(), Mismatch> {
    let fail = || Mismatch {
        path: path.to_string(),
        expected: inline(ty),
        found: found(value),
    };
    let ok = |accepted: bool| if accepted { Ok(()) } else { Err(fail()) };
    match ty {
        Ts::Number => ok(value.is_number()),
        Ts::String => ok(value.is_string()),
        Ts::Boolean => ok(value.is_boolean()),
        Ts::Null => ok(value.is_null()),
        Ts::Unknown => Ok(()),
        Ts::StrLit(literal) => ok(value.as_str() == Some(literal.as_str())),
        // `as_i64` is `None` for a float, so `1.0` is not `1`.
        Ts::IntLit(literal) => ok(value.as_i64() == Some(*literal)),
        Ts::Array(inner) => {
            let items = value.as_array().ok_or_else(fail)?;
            let path = format!("{path}[]");
            items
                .iter()
                .try_for_each(|item| walk(item, inner, &path, seen))
        }
        Ts::Object(object) => {
            let map = value.as_object().ok_or_else(fail)?;
            for (key, item) in map {
                match (object.fields.iter().find(|f| f.name == *key), &object.rest) {
                    (Some(field), _) => walk(item, &field.ty, &child(path, key), seen)?,
                    (None, Some(rest)) => {
                        let path = rest_path(path, rest);
                        seen.insert(presence_fact(&path, true));
                        walk(item, &rest.ty, &path, seen)?;
                    }
                    (None, None) => {
                        return Err(Mismatch {
                            path: child(path, key),
                            expected: "no such key".into(),
                            found: found(item),
                        });
                    }
                }
            }
            for field in &object.fields {
                let present = map.contains_key(&field.name);
                if field.optional {
                    seen.insert(presence_fact(&child(path, &field.name), present));
                } else if !present {
                    return Err(Mismatch {
                        path: child(path, &field.name),
                        expected: inline(&field.ty),
                        found: "no key".into(),
                    });
                }
            }
            Ok(())
        }
        Ts::Union(members) => {
            let mut deepest: Option<Mismatch> = None;
            for member in members {
                let mut inner = BTreeSet::new();
                match walk(value, member, path, &mut inner) {
                    Ok(()) => {
                        seen.insert(member_fact(path, member));
                        seen.extend(inner);
                        return Ok(());
                    }
                    // A member that failed inside the value (a key of an
                    // object member) says more than the whole union.
                    Err(error) if error.path.len() > path.len() => {
                        if deepest
                            .as_ref()
                            .is_none_or(|d| error.path.len() > d.path.len())
                        {
                            deepest = Some(error);
                        }
                    }
                    Err(_) => {}
                }
            }
            Err(deepest.unwrap_or_else(fail))
        }
        Ts::JsonText(inner) => {
            let text = value.as_str().ok_or_else(fail)?;
            let parsed: Value = serde_json::from_str(text).map_err(|_| Mismatch {
                path: path.to_string(),
                expected: format!("the JSON text of {}", inline(inner)),
                found: found(value),
            })?;
            walk(&parsed, inner, path, seen)
        }
        Ts::Named(named) => walk(value, &named.ty, path, seen),
    }
}

/// Every fact [`walk`] can record for `ty`.
fn facts(ty: &Ts, path: &str, out: &mut BTreeSet<String>) {
    match ty {
        Ts::Union(members) => {
            for member in members {
                out.insert(member_fact(path, member));
                facts(member, path, out);
            }
        }
        Ts::Object(object) => {
            for field in &object.fields {
                let path = child(path, &field.name);
                if field.optional {
                    out.insert(presence_fact(&path, true));
                    out.insert(presence_fact(&path, false));
                }
                facts(&field.ty, &path, out);
            }
            if let Some(rest) = &object.rest {
                let path = rest_path(path, rest);
                out.insert(presence_fact(&path, true));
                facts(&rest.ty, &path, out);
            }
        }
        Ts::Array(inner) => facts(inner, &format!("{path}[]"), out),
        Ts::JsonText(inner) => facts(inner, path, out),
        Ts::Named(named) => facts(&named.ty, path, out),
        Ts::Number
        | Ts::String
        | Ts::Boolean
        | Ts::Null
        | Ts::StrLit(_)
        | Ts::IntLit(_)
        | Ts::Unknown => {}
    }
}

/// The TypeScript type expression of `ty`, naming named types.
pub fn inline(ty: &Ts) -> String {
    match ty {
        Ts::Number => "number".into(),
        Ts::String | Ts::JsonText(_) => "string".into(),
        Ts::Boolean => "boolean".into(),
        Ts::Null => "null".into(),
        Ts::Unknown => "unknown".into(),
        // A JSON string is a TypeScript string literal.
        Ts::StrLit(literal) => Value::String(literal.clone()).to_string(),
        Ts::IntLit(literal) => literal.to_string(),
        Ts::Array(inner) => match **inner {
            Ts::Union(_) => format!("({})[]", inline(inner)),
            _ => format!("{}[]", inline(inner)),
        },
        Ts::Object(object) => {
            if object.fields.is_empty() && object.rest.is_none() {
                return "Record<string, never>".into();
            }
            let mut parts: Vec<String> = object.fields.iter().map(property).collect();
            if let Some(rest) = &object.rest {
                parts.push(index_signature(object, rest));
            }
            format!("{{ {} }}", parts.join("; "))
        }
        Ts::Union(members) => members.iter().map(inline).collect::<Vec<_>>().join(" | "),
        Ts::Named(named) => named.name.clone(),
    }
}

/// `name` as a TypeScript property key: quoted unless it is an identifier.
pub fn key(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if identifier {
        name.to_string()
    } else {
        Value::String(name.to_string()).to_string()
    }
}

fn property(field: &Field) -> String {
    let mark = if field.optional { "?" } else { "" };
    format!("{}{mark}: {}", key(&field.name), inline(&field.ty))
}

/// TypeScript wants every declared property to fit the index signature, so
/// its type joins theirs.
fn index_signature(object: &Object, rest: &Field) -> String {
    let mut members: Vec<String> = Vec::new();
    let mut add = |ty: &Ts| {
        let parts = match ty {
            Ts::Union(inner) => inner.iter().map(inline).collect(),
            other => vec![inline(other)],
        };
        for part in parts {
            if !members.contains(&part) {
                members.push(part);
            }
        }
    };
    add(&rest.ty);
    for field in &object.fields {
        add(&field.ty);
    }
    if object.fields.iter().any(|f| f.optional) {
        members.push("undefined".into());
    }
    format!("[{}: string]: {}", rest.name, members.join(" | "))
}

/// Two different types under one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict(pub String);

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "two different types are named {}", self.0)
    }
}

impl std::error::Error for Conflict {}

/// The `export` declarations of every named type reachable from `roots`, in
/// the order they are first reached.
pub fn declarations<'a>(roots: impl IntoIterator<Item = &'a Ts>) -> Result<String, Conflict> {
    let mut named: Vec<&Named> = Vec::new();
    for root in roots {
        collect(root, &mut named)?;
    }
    Ok(named
        .into_iter()
        .map(declaration)
        .collect::<Vec<_>>()
        .join("\n"))
}

fn collect<'a>(ty: &'a Ts, named: &mut Vec<&'a Named>) -> Result<(), Conflict> {
    match ty {
        Ts::Named(n) => {
            if let Some(existing) = named.iter().find(|e| e.name == n.name) {
                return if **existing == **n {
                    Ok(())
                } else {
                    Err(Conflict(n.name.clone()))
                };
            }
            named.push(n);
            collect(&n.ty, named)
        }
        Ts::Array(inner) | Ts::JsonText(inner) => collect(inner, named),
        Ts::Object(object) => {
            for field in object.fields.iter().chain(object.rest.as_deref()) {
                collect(&field.ty, named)?;
            }
            Ok(())
        }
        Ts::Union(members) => members.iter().try_for_each(|m| collect(m, named)),
        Ts::Number
        | Ts::String
        | Ts::Boolean
        | Ts::Null
        | Ts::StrLit(_)
        | Ts::IntLit(_)
        | Ts::Unknown => Ok(()),
    }
}

fn declaration(named: &Named) -> String {
    let mut out = doc_comment(&named.doc, "");
    match &named.ty {
        Ts::Object(object) if !object.fields.is_empty() || object.rest.is_some() => {
            out.push_str(&format!("export interface {} {{\n", named.name));
            for field in &object.fields {
                out.push_str(&doc_comment(&field.doc, "  "));
                out.push_str(&format!("  {};\n", property(field)));
            }
            if let Some(rest) = &object.rest {
                out.push_str(&doc_comment(&rest.doc, "  "));
                out.push_str(&format!("  {};\n", index_signature(object, rest)));
            }
            out.push_str("}\n");
        }
        other => out.push_str(&format!(
            "export type {} = {};\n",
            named.name,
            inline(other)
        )),
    }
    out
}

fn doc_comment(doc: &str, indent: &str) -> String {
    let lines: Vec<&str> = doc.lines().collect();
    match lines.as_slice() {
        [] => String::new(),
        [line] => format!("{indent}/** {line} */\n"),
        lines => {
            let mut out = format!("{indent}/**\n");
            for line in lines {
                if line.is_empty() {
                    out.push_str(&format!("{indent} *\n"));
                } else {
                    out.push_str(&format!("{indent} * {line}\n"));
                }
            }
            out.push_str(&format!("{indent} */\n"));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading() -> Ts {
        Ts::named("Reading", "", Ts::Number.or(Ts::literal("NaN")).nullable())
    }

    fn payload() -> Ts {
        Ts::named(
            "Payload",
            "A payload.",
            Ts::object([
                field("p", reading()),
                field("kind", Ts::literals(["a", "b"])),
                field("v", Ts::IntLit(1)),
                field("tags", Ts::String.array()).optional(),
                field("detail", Ts::object([field("n", Ts::Number)]).nullable()).doc("Two\nlines."),
            ]),
        )
    }

    #[test]
    fn a_value_of_the_type_passes() {
        let value = json!({"p": "NaN", "kind": "a", "v": 1, "detail": {"n": 2.5}});
        assert_eq!(check("x", &value, &payload()), Ok(()));
    }

    #[test]
    fn mismatches_name_the_path() {
        let cases = [
            (
                json!({"p": true, "kind": "a", "v": 1, "detail": null}),
                "x.p",
            ),
            (
                json!({"p": 1, "kind": "c", "v": 1, "detail": null}),
                "x.kind",
            ),
            (
                json!({"p": 1, "kind": "a", "v": 1.0, "detail": null}),
                "x.v",
            ),
            (json!({"p": 1, "kind": "a", "v": 1}), "x.detail"),
            (
                json!({"p": 1, "kind": "a", "v": 1, "detail": null, "extra": 0}),
                "x.extra",
            ),
            (
                json!({"p": 1, "kind": "a", "v": 1, "detail": {"n": "1"}}),
                "x.detail.n",
            ),
            (
                json!({"p": 1, "kind": "a", "v": 1, "detail": null, "tags": [1]}),
                "x.tags[]",
            ),
        ];
        for (value, path) in cases {
            let error = check("x", &value, &payload()).unwrap_err();
            assert_eq!(error.path, path, "{value}: {error}");
        }
    }

    #[test]
    fn json_text_holds_its_type_inside_a_string() {
        let ty = Ts::object([field("id", Ts::String)]).json_text();
        assert_eq!(check("n", &json!("{\"id\": \"1\"}"), &ty), Ok(()));
        assert_eq!(
            check("n", &json!("{\"id\": 1}"), &ty).unwrap_err().path,
            "n.id"
        );
        assert!(check("n", &json!({"id": "1"}), &ty).is_err());
    }

    #[test]
    fn coverage_lists_what_no_value_exercised() {
        let ty = payload();
        let mut coverage = Coverage::new("x", &ty);
        coverage
            .check(&json!({"p": 1, "kind": "a", "v": 1, "detail": null}))
            .unwrap();
        assert_eq!(
            coverage.missing(),
            [
                "x.detail: { n: number }",
                "x.kind: \"b\"",
                "x.p: \"NaN\"",
                "x.p: null",
                "x.tags present",
            ]
        );
        coverage
            .check(&json!({"p": "NaN", "kind": "b", "v": 1, "detail": {"n": 0}, "tags": []}))
            .unwrap();
        coverage
            .check(&json!({"p": null, "kind": "b", "v": 1, "detail": null}))
            .unwrap();
        assert_eq!(coverage.missing(), Vec::<String>::new());
        assert_eq!(coverage.values(), 3);
    }

    #[test]
    fn index_signatures_take_other_keys() {
        let ty = Ts::object_with_rest(
            [
                field("active", Ts::String.nullable()),
                field("flow", Ts::Number).optional(),
            ],
            field("kind", Ts::Number),
        );
        let mut coverage = Coverage::new("s", &ty);
        coverage.check(&json!({"active": null})).unwrap();
        assert_eq!(
            coverage.missing(),
            ["s.active: string", "s.flow present", "s[kind] present"]
        );
        coverage
            .check(&json!({"active": "x", "flow": 1, "x": 2}))
            .unwrap();
        assert_eq!(coverage.missing(), Vec::<String>::new());
        assert!(check("s", &json!({"active": null, "x": "y"}), &ty).is_err());
        assert_eq!(
            inline(&ty),
            "{ active: string | null; flow?: number; [kind: string]: number | string | null | undefined }"
        );
    }

    #[test]
    fn declarations_export_each_named_type_once() {
        let ty = payload();
        let event = Ts::named("Other", "", Ts::object([field("p", reading())]));
        let text = declarations([&ty, &event, &Ts::named("Empty", "", Ts::object([]))]).unwrap();
        assert_eq!(
            text,
            r#"/** A payload. */
export interface Payload {
  p: Reading;
  kind: "a" | "b";
  v: 1;
  tags?: string[];
  /**
   * Two
   * lines.
   */
  detail: { n: number } | null;
}

export type Reading = number | "NaN" | null;

export interface Other {
  p: Reading;
}

export type Empty = Record<string, never>;
"#
        );
    }

    #[test]
    fn a_name_used_for_two_types_is_a_conflict() {
        let a = Ts::named("A", "", Ts::Number);
        let b = Ts::named("A", "", Ts::String);
        assert_eq!(declarations([&a, &b]), Err(Conflict("A".into())));
    }

    #[test]
    fn keys_that_are_not_identifiers_are_quoted() {
        let ty = Ts::object([field("a-b", Ts::Number), field("final", Ts::Boolean)]);
        assert_eq!(inline(&ty), "{ \"a-b\": number; final: boolean }");
    }
}
