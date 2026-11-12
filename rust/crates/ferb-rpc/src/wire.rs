//! The frames of FERB's local RPC: one JSON object per line, in both directions.
//!
//! Port of the wire part of `ferb/daemon/server.py` (`Request`, `Hello`, `_line`, `_result`, `_error`, the event
//! frame of `_stream`). What the daemon writes is byte for byte what Python's `json.dumps(msg).encode() + b"\n"`
//! writes (server.py:451-452): `", "` and `": "` separators, every character outside printable ASCII as `\uXXXX`,
//! floats as Python's `repr`. What it accepts and the words it refuses with are those of `json.loads` followed by
//! pydantic's `model_validate` (server.py:235-242), as recorded in `rust/fixtures/rpc/frames.json` (ENG-3).

use std::io;

use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::ser::{CharEscape, Formatter};
use serde_json::{Map, Value};

use crate::auth::Role;

/// The largest message, without its newline (server.py:26). Longer: `TOO_LARGE`, and the connection ends.
pub const MAX_MESSAGE: usize = 1024 * 1024;

// server.py:28-31
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INTERNAL: i64 = -32603;
pub const UNAUTHORIZED: i64 = -32001;
pub const TOO_LARGE: i64 = -32002;
pub const UNSUPPORTED: i64 = -32003;
pub const FAILED: i64 = -32004;
/// A FERB.app-only method called by another program (auth.APP_ONLY): its own code, so the app tells it from a
/// daemon too old to know the method (server.py:30-31; DaemonRPC.swift reads the code).
pub const APP_ONLY_REFUSED: i64 = -32005;

/// A request's or stream's parameters and a call's result: always a JSON object (`dict[str, Any]`).
pub type Params = Map<String, Value>;

/// A request id as pydantic's `int | str | None` keeps it (server.py:58): `4.0`, `true` and `false` arrive as the
/// integers 4, 1 and 0, text stays text, and a missing id is null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Id {
    Null,
    Int(serde_json::Number),
    Str(String),
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Id::Null => s.serialize_unit(),
            Id::Int(n) => n.serialize(s),
            Id::Str(text) => s.serialize_str(text),
        }
    }
}

/// One request (server.py:56-60).
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: Id,
    pub method: String,
    pub params: Params,
}

/// `auth.hello`'s parameters (server.py:63-66).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub role: Role,
    pub token: Option<String>,
    /// native_host: the caller extension's origin.
    pub origin: Option<String>,
}

/// Why a line is not a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Not JSON: `PARSE_ERROR` "invalid JSON", and the connection keeps serving (server.py:237-239).
    Parse,
    /// JSON, but not a request: `INVALID_REQUEST` with pydantic's first message (server.py:240-242).
    Invalid(&'static str),
    /// Bytes that are not UTF-8. Python's `json.loads` raises `UnicodeDecodeError`, which `_connection` does not
    /// catch (server.py:236, 253): the connection ends without an answer.
    NotText,
}

const NOT_AN_OBJECT: &str = "Input should be a valid dictionary or instance of Request";
const REQUIRED: &str = "Field required";
const NOT_2_0: &str = "Input should be '2.0'";
const NOT_AN_INT: &str = "Input should be a valid integer";
const FRACTIONAL: &str = "Input should be a valid integer, got a number with a fractional part";
const NOT_A_STRING: &str = "Input should be a valid string";
const NOT_A_DICT: &str = "Input should be a valid dictionary";
const NOT_A_ROLE: &str = "Input should be 'cli', 'mcp_shim' or 'native_host'";

/// One received line (with or without its newline) as a request. The checks run in the model's field order
/// (jsonrpc, id, method, params) and the first failure's message is the answer, as `e.errors()[0]["msg"]` is.
pub fn parse_request(line: &[u8]) -> Result<Request, Reject> {
    // json.loads(bytes) reads a UTF-8 byte-order mark as "utf-8-sig"
    let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
    let text = std::str::from_utf8(line).map_err(|_| Reject::NotText)?;
    let value: Value = serde_json::from_str(text).map_err(|_| Reject::Parse)?;
    let Value::Object(mut object) = value else {
        return Err(Reject::Invalid(NOT_AN_OBJECT));
    };
    match object.get("jsonrpc") {
        None => return Err(Reject::Invalid(REQUIRED)),
        Some(Value::String(version)) if version == "2.0" => {}
        Some(_) => return Err(Reject::Invalid(NOT_2_0)),
    }
    let id = match object.remove("id") {
        None | Some(Value::Null) => Id::Null,
        Some(Value::String(text)) => Id::Str(text),
        Some(Value::Bool(flag)) => Id::Int(u8::from(flag).into()),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Id::Int(n),
        Some(Value::Number(n)) => Id::Int(whole(n.as_f64().unwrap_or(f64::NAN))?),
        Some(_) => return Err(Reject::Invalid(NOT_AN_INT)),
    };
    let method = match object.remove("method") {
        None => return Err(Reject::Invalid(REQUIRED)),
        Some(Value::String(method)) => method,
        Some(_) => return Err(Reject::Invalid(NOT_A_STRING)),
    };
    let params = match object.remove("params") {
        None => Params::new(),
        Some(Value::Object(params)) => params,
        Some(_) => return Err(Reject::Invalid(NOT_A_DICT)),
    };
    Ok(Request { id, method, params })
}

/// A float id: pydantic takes a whole one as that integer. One outside what a JSON integer holds here (below
/// -2^63 or above 2^64-1) is refused, where Python would echo the big integer; no client sends one.
fn whole(f: f64) -> Result<serde_json::Number, Reject> {
    if f.fract() != 0.0 {
        return Err(Reject::Invalid(FRACTIONAL));
    }
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) {
        Ok((f as i64).into())
    } else if (0.0..18_446_744_073_709_551_616.0).contains(&f) {
        Ok((f as u64).into())
    } else {
        Err(Reject::Invalid(NOT_AN_INT))
    }
}

/// `Hello.model_validate(req.params)` (server.py:264): the first failing field's message, in the order role,
/// token, origin. Unknown fields are ignored.
pub fn parse_hello(params: &Params) -> Result<Hello, &'static str> {
    let role = match params.get("role") {
        None => return Err(REQUIRED),
        Some(Value::String(role)) => Role::from_wire(role).ok_or(NOT_A_ROLE)?,
        Some(_) => return Err(NOT_A_ROLE),
    };
    let text = |key: &str| match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(NOT_A_STRING),
    };
    Ok(Hello {
        role,
        token: text("token")?,
        origin: text("origin")?,
    })
}

// ---------- what the daemon writes ----------

/// `{"jsonrpc": "2.0", "id": …, "result": {…}}` (server.py:455-456).
#[derive(Clone, Debug, Serialize)]
pub struct ResultFrame<'a> {
    pub jsonrpc: &'static str,
    pub id: &'a Id,
    pub result: &'a Params,
}

/// The `error` member of an error frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: i64,
    pub message: String,
}

/// `{"jsonrpc": "2.0", "id": …, "error": {"code": …, "message": …}}` (server.py:459-460).
#[derive(Clone, Debug, Serialize)]
pub struct ErrorFrame<'a> {
    pub jsonrpc: &'static str,
    pub id: &'a Id,
    pub error: ErrorBody,
}

/// A stream's notification, `{"jsonrpc": "2.0", "method": "<method>.event", "params": {…}}` (server.py:415).
/// It carries no id.
#[derive(Clone, Debug, Serialize)]
pub struct EventFrame<'a> {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: &'a Params,
}

/// The line of a result.
pub fn result_line(id: &Id, result: &Params) -> Vec<u8> {
    line(&ResultFrame {
        jsonrpc: "2.0",
        id,
        result,
    })
}

/// The line of an error.
pub fn error_line(id: &Id, code: i64, message: &str) -> Vec<u8> {
    line(&ErrorFrame {
        jsonrpc: "2.0",
        id,
        error: ErrorBody {
            code,
            message: message.to_owned(),
        },
    })
}

/// The line of one stream event of `method`.
pub fn event_line(method: &str, params: &Params) -> Vec<u8> {
    line(&EventFrame {
        jsonrpc: "2.0",
        method: format!("{method}.event"),
        params,
    })
}

/// A stream's last frame when it ended by itself: the result `{"ended": true}` (server.py:428).
pub fn ended_line(id: &Id) -> Vec<u8> {
    let mut ended = Params::new();
    ended.insert("ended".to_owned(), Value::Bool(true));
    result_line(id, &ended)
}

/// `json.dumps(value).encode() + b"\n"` (server.py:451-452).
pub fn line<T: Serialize>(value: &T) -> Vec<u8> {
    let mut out = dumps(value).into_bytes();
    out.push(b'\n');
    out
}

/// `json.dumps(value)` with Python's defaults.
pub fn dumps<T: Serialize>(value: &T) -> String {
    let mut out = Vec::with_capacity(128);
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, PythonJson);
    // Writing JSON values and string-keyed maps into a Vec cannot fail.
    value
        .serialize(&mut serializer)
        .expect("a frame is always JSON");
    String::from_utf8(out).expect("the formatter writes ASCII only")
}

/// A reply or a stream event as a client reads it (`ferb/cli/client.py:57-80`, `DaemonRPC.swift`): which one is
/// told by the members present.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Incoming {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Params>,
    #[serde(default)]
    pub result: Option<Params>,
    #[serde(default)]
    pub error: Option<ErrorBody>,
}

/// A request line as the clients write it (`ferb/cli/client.py:36-37`): id, method, params.
pub fn request_line(id: i64, method: &str, params: &Params) -> Vec<u8> {
    struct Out<'a>(i64, &'a str, &'a Params);
    impl Serialize for Out<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut map = s.serialize_map(Some(4))?;
            map.serialize_entry("jsonrpc", "2.0")?;
            map.serialize_entry("id", &self.0)?;
            map.serialize_entry("method", self.1)?;
            map.serialize_entry("params", self.2)?;
            map.end()
        }
    }
    line(&Out(id, method, params))
}

/// `json.dumps`'s output format: `, ` between items, `: ` after a key, ASCII only, floats as `repr(float)`.
struct PythonJson;

impl Formatter for PythonJson {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(python_float(value).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        w.write_all(python_float(f64::from(value)).as_bytes())
    }

    /// Python's `ensure_ascii`: everything outside `' '..='~'` is `\uXXXX` (a pair for characters above U+FFFF).
    /// The quote, the backslash and the control characters never reach here (`write_char_escape` has them), but
    /// U+007F does.
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut plain = 0;
        for (at, ch) in fragment.char_indices() {
            if (' '..='~').contains(&ch) {
                continue;
            }
            w.write_all(&fragment.as_bytes()[plain..at])?;
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                write!(w, "\\u{unit:04x}")?;
            }
            plain = at + ch.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[plain..])
    }

    /// As serde_json writes them, which is as Python does: `\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`, and
    /// `\u00xx` in lowercase for the other control characters. `/` is not escaped.
    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        match escape {
            CharEscape::Quote => w.write_all(b"\\\""),
            CharEscape::ReverseSolidus => w.write_all(b"\\\\"),
            CharEscape::Solidus => w.write_all(b"/"),
            CharEscape::Backspace => w.write_all(b"\\b"),
            CharEscape::FormFeed => w.write_all(b"\\f"),
            CharEscape::LineFeed => w.write_all(b"\\n"),
            CharEscape::CarriageReturn => w.write_all(b"\\r"),
            CharEscape::Tab => w.write_all(b"\\t"),
            CharEscape::AsciiControl(byte) => write!(w, "\\u{byte:04x}"),
        }
    }
}

/// `repr(float)`: the shortest digits that read back as the same number; scientific notation when the decimal
/// exponent is below -4 or at least 16, with a sign and at least two exponent digits; otherwise plain with a
/// fractional part (`2.0`). A serde_json number is never NaN or infinite.
fn python_float(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    // Rust's `{:e}` gives the same shortest digits: "-1.2345e-7"
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("{:e} always has an exponent");
    let exponent: i32 = exponent.parse().expect("{:e} writes a decimal exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if value < 0.0 { "-" } else { "" };
    if !(-4..16).contains(&exponent) {
        let mark = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{mantissa}e{mark}{:02}", exponent.abs());
    }
    if exponent < 0 {
        let zeros = "0".repeat((-exponent - 1) as usize);
        return format!("{sign}0.{zeros}{digits}");
    }
    let whole = exponent as usize + 1;
    if digits.len() <= whole {
        let zeros = "0".repeat(whole - digits.len());
        format!("{sign}{digits}{zeros}.0")
    } else {
        format!("{sign}{}.{}", &digits[..whole], &digits[whole..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_are_written_as_python_writes_them() {
        for (value, text) in [
            (1.5, "1.5"),
            (2.0, "2.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e-7, "1e-07"),
            (-2.5e-5, "-2.5e-05"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (123456789012345.0, "123456789012345.0"),
            (1234567890123456.0, "1234567890123456.0"),
            (12345678901234567.0, "1.2345678901234568e+16"),
            (6.02214076e23, "6.02214076e+23"),
            (1e300, "1e+300"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (100.0, "100.0"),
            (-12.75, "-12.75"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(python_float(value), text);
        }
    }

    #[test]
    fn text_is_ascii_only_with_python_s_escapes() {
        let value =
            json!({"k\u{e9}": "a\u{7f}\u{1}\u{1f600}\"\\/\n", "list": [1, null, true], "o": {}});
        assert_eq!(
            dumps(&value),
            "{\"k\\u00e9\": \"a\\u007f\\u0001\\ud83d\\ude00\\\"\\\\/\\n\", \"list\": [1, null, true], \"o\": {}}"
        );
    }

    #[test]
    fn an_id_is_kept_as_pydantic_keeps_it() {
        let id = |text: &str| parse_request(text.as_bytes()).map(|r| r.id);
        assert_eq!(id(r#"{"jsonrpc":"2.0","method":"m"}"#), Ok(Id::Null));
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":4.0,"method":"m"}"#),
            Ok(Id::Int(4.into()))
        );
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":true,"method":"m"}"#),
            Ok(Id::Int(1.into()))
        );
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":"7","method":"m"}"#),
            Ok(Id::Str("7".into()))
        );
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":1.5,"method":"m"}"#),
            Err(Reject::Invalid(FRACTIONAL))
        );
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":1e30,"method":"m"}"#),
            Err(Reject::Invalid(NOT_AN_INT))
        );
    }

    #[test]
    fn a_client_request_line_is_what_the_python_client_writes() {
        let mut params = Params::new();
        params.insert("role".into(), json!("cli"));
        assert_eq!(
            request_line(1, "auth.hello", &params),
            b"{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"auth.hello\", \"params\": {\"role\": \"cli\"}}\n"
        );
    }
}
