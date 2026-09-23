//! Minimal JSON reader (std-only; no external crates).
//!
//! Only what dcheck needs to read `smartctl -j` output. Strings are treated as
//! byte sequences (smartctl emits ASCII in practice); numbers are `f64`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn parse(input: &str) -> Option<Json> {
        let mut p = Parser {
            bytes: input.as_bytes(),
            pos: 0,
        };
        p.ws();
        let value = p.value()?;
        p.ws();
        Some(value)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_f64().map(|n| n.max(0.0) as u64)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|n| n as i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Json::Str),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'n' => self.literal("null", Json::Null),
            _ => self.number(),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Option<Json> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Some(value)
        } else {
            None
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.pos += 1; // '{'
        let mut map = BTreeMap::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(Json::Obj(map));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            let value = self.value()?;
            map.insert(key, value);
            self.ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(Json::Obj(map))
    }

    fn array(&mut self) -> Option<Json> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(Json::Arr(items));
        }
        loop {
            let value = self.value()?;
            items.push(value);
            self.ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(Json::Arr(items))
    }

    fn string(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        self.pos += 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            match c {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek()?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return None;
                            }
                            let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4]).ok()?;
                            let cp = u32::from_str_radix(hex, 16).ok()?;
                            self.pos += 4;
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        _ => return None,
                    }
                }
                _ => {
                    out.push(c as char);
                    self.pos += 1;
                }
            }
        }
        None
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse::<f64>()
            .ok()
            .map(Json::Num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_values() {
        let j = Json::parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5}}"#).unwrap();
        assert_eq!(j.get("a").and_then(Json::as_u64), Some(1));
        assert_eq!(j.get("c").and_then(|c| c.get("d")).and_then(Json::as_f64), Some(-2.5));
        let arr = j.get("b").and_then(Json::as_array).unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_bool(), Some(true));
        assert_eq!(arr[2].as_str(), Some("x"));
    }

    #[test]
    fn handles_escapes_and_whitespace() {
        let j = Json::parse("  { \"k\" : \"a\\tb\\n\" }  ").unwrap();
        assert_eq!(j.get("k").and_then(Json::as_str), Some("a\tb\n"));
    }
}
