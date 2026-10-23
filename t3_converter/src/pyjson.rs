//! JSON written byte for byte as Python's `json` module writes it (the Python converter's output is the reference):
//! `dumps(o, separators=(',', ':'))`, `dump(o, f, indent=1)`, the default separators, `sort_keys`, `ensure_ascii`.
//! Values are `serde_json::Value` with `preserve_order` (a Python dict's insertion order); a float is written as
//! Python's `repr(float)`, an integer as an integer.

use serde_json::Value;

/// How to write: the item and key separators, an indent (None: one line), sorted keys, ASCII only.
#[derive(Clone, Copy)]
pub struct Style {
    pub item: &'static str,
    pub key: &'static str,
    pub indent: Option<usize>,
    pub sort_keys: bool,
    pub ensure_ascii: bool,
}

/// `json.dumps(o, separators=(',', ':'))`
pub const COMPACT: Style = Style { item: ",", key: ":", indent: None, sort_keys: false, ensure_ascii: true };
/// `json.dump(o, f, indent=1)` (Python's item separator is ',' once there is an indent)
pub const INDENT1: Style = Style { item: ",", key: ": ", indent: Some(1), sort_keys: false, ensure_ascii: true };
/// `json.dumps(o)`
pub const DEFAULT: Style = Style { item: ", ", key: ": ", indent: None, sort_keys: false, ensure_ascii: true };
/// `json.dumps(o, sort_keys=True, separators=(',', ':'))` (the fingerprint's canonical form)
pub const CANONICAL: Style = Style { item: ",", key: ":", indent: None, sort_keys: true, ensure_ascii: true };

pub fn to_string(v: &Value, style: Style) -> String {
    let mut out = String::new();
    write_value(&mut out, v, style, 0);
    out
}

/// `json.dumps(o, separators=(',', ':'))`
pub fn compact(v: &Value) -> String {
    to_string(v, COMPACT)
}

/// `json.dump(o, f, indent=1)`
pub fn indent1(v: &Value) -> String {
    to_string(v, INDENT1)
}

fn newline(out: &mut String, style: Style, level: usize) {
    if let Some(n) = style.indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', n * level));
    }
}

fn write_value(out: &mut String, v: &Value, style: Style, level: usize) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&float_repr(n.as_f64().unwrap_or(f64::NAN)));
            }
        }
        Value::String(s) => write_str(out, s, style.ensure_ascii),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item);
                }
                newline(out, style, level + 1);
                write_value(out, x, style, level + 1);
            }
            newline(out, style, level);
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut items: Vec<(&String, &Value)> = m.iter().collect();
            if style.sort_keys {
                items.sort_by(|a, b| a.0.cmp(b.0));
            }
            for (i, (k, x)) in items.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item);
                }
                newline(out, style, level + 1);
                write_str(out, k, style.ensure_ascii);
                out.push_str(style.key);
                write_value(out, x, style, level + 1);
            }
            newline(out, style, level);
            out.push('}');
        }
    }
}

/// A string as Python's encoder writes it.
pub fn write_str(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (ensure_ascii && (c as u32) > 0x7e) => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(float)` (and `json`'s float): the shortest digits that round-trip, positional when the decimal
/// exponent is in -4..16, else `d.ddde+XX`; whole numbers keep `.0`.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust's `{:e}` is the shortest round-trip form too: "-1.2345e-5"
    let mut e = format!("{:e}", f);
    // on a tie between two shortest candidates Rust rounds half up, Python half to even (as exact formatting does)
    let digits = e.split('e').next().unwrap_or("").bytes().filter(u8::is_ascii_digit).count();
    let even = format!("{:.*e}", digits.saturating_sub(1), f);
    if even.parse::<f64>().ok() == Some(f) {
        e = even;
    }
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mant) = mant.strip_prefix('-').map_or(("", mant), |m| ("-", m));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let n = digits.len() as i32;
    if (-4..16).contains(&exp) {
        let point = exp + 1; // digits before the point
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= n {
            format!("{}{}.0", digits, "0".repeat((point - n) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{s}")
    } else {
        let m = if n > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        format!("{sign}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_as_python() {
        let cases = [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (-0.0, "-0.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1.5e16, "1.5e+16"),
            (1e15, "1000000000000000.0"),
            (123.456, "123.456"),
            (1e100, "1e+100"),
            (-2.5e-7, "-2.5e-07"),
            (0.30000000000000004, "0.30000000000000004"),
            (f32::MAX as f64, "3.4028234663852886e+38"),
            (13.472f32 as f64, "13.472000122070312"),
        ];
        for (f, s) in cases {
            assert_eq!(float_repr(f), s, "{f}");
        }
    }

    #[test]
    fn layouts_as_python() {
        let v = json!({"b": [1, 2.0, {"x": "é\n"}], "a": [], "c": {}});
        assert_eq!(compact(&v), r#"{"b":[1,2.0,{"x":"\u00e9\n"}],"a":[],"c":{}}"#);
        assert_eq!(
            indent1(&v),
            "{\n \"b\": [\n  1,\n  2.0,\n  {\n   \"x\": \"\\u00e9\\n\"\n  }\n ],\n \"a\": [],\n \"c\": {}\n}"
        );
        assert_eq!(to_string(&v, CANONICAL), r#"{"a":[],"b":[1,2.0,{"x":"\u00e9\n"}],"c":{}}"#);
        assert_eq!(to_string(&json!([1, "x"]), DEFAULT), r#"[1, "x"]"#);
    }
}
