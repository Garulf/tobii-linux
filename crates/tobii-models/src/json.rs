//! The JSON manifests of the models/tobii-et5 layout, written byte for byte as Python's
//! `json.dumps(value, indent=2) + '\n'` writes them (key order kept, ASCII-only output).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn object<K: Into<String>>(members: impl IntoIterator<Item = (K, Json)>) -> Self {
        Self::Object(members.into_iter().map(|(key, value)| (key.into(), value)).collect())
    }

    pub fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    pub fn int(value: impl TryInto<i64>) -> Self {
        Self::Int(value.try_into().ok().expect("JSON integer within i64"))
    }

    /// `None` as `null`, as Python writes a missing attribute.
    pub fn optional(text: Option<&str>) -> Self {
        text.map_or(Self::Null, Self::str)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The file contents: the indented document and a final newline.
    pub fn to_file(&self) -> Vec<u8> {
        let mut text = String::new();
        self.write(&mut text, 0);
        text.push('\n');
        text.into_bytes()
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Int(value) => out.push_str(&value.to_string()),
            Self::Str(text) => quote(out, text),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Object(members) if members.is_empty() => out.push_str("{}"),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    separator(out, i, depth + 1);
                    item.write(out, depth + 1);
                }
                close(out, depth, ']');
            }
            Self::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    separator(out, i, depth + 1);
                    quote(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                close(out, depth, '}');
            }
        }
    }
}

fn separator(out: &mut String, index: usize, depth: usize) {
    if index > 0 {
        out.push(',');
    }
    out.push('\n');
    out.push_str(&"  ".repeat(depth));
}

fn close(out: &mut String, depth: usize, bracket: char) {
    out.push('\n');
    out.push_str(&"  ".repeat(depth));
    out.push(bracket);
}

fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python_json_dumps_indent_2() {
        let value = Json::object([
            ("a", Json::Array(vec![Json::int(1), Json::Array(vec![]), Json::object::<&str>([])])),
            ("b", Json::object([("c", Json::Null), ("d", Json::Bool(true))])),
            ("e\u{e9}\u{1f600}\"\\\n\u{1}", Json::str("/x\u{7f}")),
        ]);
        let expected = "{\n  \"a\": [\n    1,\n    [],\n    {}\n  ],\n  \"b\": {\n    \"c\": null,\n    \"d\": true\n  },\n  \
                        \"e\\u00e9\\ud83d\\ude00\\\"\\\\\\n\\u0001\": \"/x\\u007f\"\n}\n";
        assert_eq!(String::from_utf8(value.to_file()).unwrap(), expected);
    }
}
