//! `met-config.py`: read and change the settings file from a shell.
//!
//! meticulous-machine's hawkbit setup runs `met-config get .system.serial`
//! and uses what it prints, so the output follows the Python's: strings
//! bare, everything else as `pprint` shows it, or JSON with `--json`.

use serde_json::Value;

use crate::config::{self, ConfigStore};
use crate::env::Env;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Output in JSON format
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, clap::Subcommand)]
enum Cmd {
    /// Retrieve a configuration value.
    Get {
        /// Dot-separated key path (e.g., machine.version)
        keypath: String,
    },
    /// Set a configuration value.
    Set {
        /// Dot-separated key path (e.g., machine.version)
        keypath: String,
        /// Value to set
        value: String,
    },
}

pub fn run(args: Args) -> std::process::ExitCode {
    let env = Env::from_process();
    match execute(&env, &args) {
        Ok(output) => {
            println!("{output}");
            std::process::ExitCode::SUCCESS
        }
        Err(message) => {
            println!("Error: {message}");
            std::process::ExitCode::from(1)
        }
    }
}

fn execute(env: &Env, args: &Args) -> Result<String, String> {
    match &args.command {
        Cmd::Get { keypath } => {
            let keys = parse_path(keypath);
            let store = ConfigStore::open(env.paths.config_file(), config::defaults());
            let tree = store.snapshot();
            let result = match keys {
                None => tree,
                Some(keys) => get_nested(&tree, &keys)?.clone(),
            };
            Ok(render(&result, args.json))
        }
        Cmd::Set { keypath, value } => {
            let keys = parse_path(keypath).ok_or("Cannot set the root of the configuration.")?;
            let store = ConfigStore::open(env.paths.config_file(), config::defaults());
            let section = keys
                .first()
                .ok_or("Cannot set the root of the configuration.")?;
            let key = keys
                .get(1)
                .ok_or_else(|| "list index out of range".to_string())?;
            let target = store.snapshot();
            let current = target
                .get(section)
                .ok_or_else(|| format!("'{section}'"))?
                .get(key)
                .ok_or_else(|| format!("'{key}'"))?
                .clone();
            let new_value = coerce(&current, value)?;
            store.set(section, key, new_value.clone());
            let shown = render(&new_value, args.json);
            store.save();
            if args.json {
                Ok(shown)
            } else {
                Ok(format!("{} =  {shown}", keys.join(".")))
            }
        }
    }
}

/// `parsePath`: a leading dot is optional; `.` alone is the whole config.
fn parse_path(path: &str) -> Option<Vec<String>> {
    let mut keys: Vec<String> = path.split('.').map(str::to_string).collect();
    if keys.first().is_some_and(|k| k.is_empty()) {
        keys.remove(0);
    }
    match keys.first() {
        None => None,
        Some(k) if k.is_empty() => None,
        Some(_) => Some(keys),
    }
}

fn get_nested<'a>(tree: &'a Value, keys: &[String]) -> Result<&'a Value, String> {
    let mut node = tree;
    for key in keys {
        match node.get(key) {
            Some(child) if node.is_object() => node = child,
            _ => {
                let allowed = node
                    .as_object()
                    .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                return Err(format!("\"Key '{key}' not in [{allowed}]\""));
            }
        }
    }
    Ok(node)
}

/// `set_nested`: the new value takes the type of the stored one.
fn coerce(current: &Value, value: &str) -> Result<Value, String> {
    match current {
        Value::Bool(_) => Ok(Value::Bool(matches!(
            value.to_lowercase().as_str(),
            "yes" | "true" | "t" | "1"
        ))),
        Value::String(_) => Ok(Value::String(value.to_string())),
        Value::Number(n) if n.is_f64() => pyfmt::py_float(value)
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("could not convert string to float: '{value}'")),
        Value::Number(_) => pyfmt::py_int(value)
            .map(Value::from)
            .ok_or_else(|| format!("invalid literal for int() with base 10: '{value}'")),
        Value::Array(_) | Value::Object(_) => {
            // Shell-quoted JSON often arrives with single quotes.
            let text = if value.contains('\'') && !value.contains('"') {
                value.replace('\'', "\"")
            } else {
                value.to_string()
            };
            serde_json::from_str(&text).map_err(|e| e.to_string())
        }
        Value::Null => Err("('Unsupported target type: ', <class 'NoneType'>)".to_string()),
    }
}

fn render(value: &Value, json: bool) -> String {
    if json {
        return pyfmt::dumps(value, &pyfmt::DumpOpts::indented(2));
    }
    match value {
        Value::String(s) => s.clone(),
        other => pformat(other, 0, 80),
    }
}

/// `repr()` of a JSON-like Python value.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) if n.is_f64() => pyfmt::repr_f64(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_str_repr(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}: {}", py_str_repr(k), py_repr(&map[k])))
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `pprint.pformat`: one line when it fits, otherwise one entry per line
/// with dictionaries sorted by key.
fn pformat(value: &Value, indent: usize, width: usize) -> String {
    let flat = py_repr(value);
    if flat.len() + indent <= width {
        return flat;
    }
    let pad = " ".repeat(indent + 1);
    match value {
        Value::Array(items) if !items.is_empty() => {
            let parts: Vec<String> = items
                .iter()
                .map(|v| pformat(v, indent + 1, width))
                .collect();
            format!("[{}]", parts.join(&format!(",\n{pad}")))
        }
        Value::Object(map) if !map.is_empty() => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = py_str_repr(k);
                    let inner = pformat(&map[k], indent + key.len() + 3, width);
                    format!("{key}: {inner}")
                })
                .collect();
            format!("{{{}}}", parts.join(&format!(",\n{pad}")))
        }
        _ => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths() {
        assert_eq!(
            parse_path(".system.serial"),
            Some(vec!["system".into(), "serial".into()])
        );
        assert_eq!(
            parse_path("system.serial"),
            Some(vec!["system".into(), "serial".into()])
        );
        assert_eq!(parse_path("."), None);
    }

    #[test]
    fn reprs_like_python() {
        assert_eq!(py_repr(&json!(null)), "None");
        assert_eq!(
            py_repr(&json!({"b": [1, 2.5, true], "a": "x"})),
            "{'a': 'x', 'b': [1, 2.5, True]}"
        );
        assert_eq!(py_repr(&json!("it's")), "\"it's\"");
    }

    #[test]
    fn coercion_follows_the_stored_type() {
        assert_eq!(coerce(&json!(true), "no").ok(), Some(json!(false)));
        assert_eq!(coerce(&json!(true), "T").ok(), Some(json!(true)));
        assert_eq!(coerce(&json!(10), "12").ok(), Some(json!(12)));
        assert_eq!(coerce(&json!(1.5), "2").ok(), Some(json!(2.0)));
        assert_eq!(coerce(&json!([]), "['a']").ok(), Some(json!(["a"])));
        assert!(coerce(&json!(null), "x").is_err());
    }

    #[test]
    fn get_and_set_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let env = Env::for_tests(dir.path(), crate::env::BackendMode::Emulator);
        let get = |path: &str| {
            execute(
                &env,
                &Args {
                    json: false,
                    command: Cmd::Get {
                        keypath: path.into(),
                    },
                },
            )
        };
        assert_eq!(get(".system.serial").as_deref(), Ok("None"));
        let set = execute(
            &env,
            &Args {
                json: false,
                command: Cmd::Set {
                    keypath: ".system.serial".into(),
                    value: "x".into(),
                },
            },
        );
        // A None target cannot be set, like the Python.
        assert!(set.is_err());
        let set = execute(
            &env,
            &Args {
                json: false,
                command: Cmd::Set {
                    keypath: "user.heating_timeout".into(),
                    value: "20".into(),
                },
            },
        );
        assert_eq!(set.as_deref(), Ok("user.heating_timeout =  20"));
        assert_eq!(get("user.heating_timeout").as_deref(), Ok("20"));
        assert!(get("user.nope").is_err());
    }
}
