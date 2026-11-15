//! The authenticated newline-JSON socket client (ferb/cli/client.py): `auth.hello` with a role, then calls and
//! streams. Frames are written as Python's `json.dumps` writes them.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::{CliError, Rpc};

pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    timeout: Option<f64>,
}

const NOT_RUNNING: &str = "FERB daemon is not running (start it with `ferb daemon run`)";

impl Client {
    /// client.py:18: connects and says hello as `role` (with the run token for the tool bridge).
    pub fn connect(
        sock_path: &Path,
        role: &str,
        token: Option<&str>,
        timeout: f64,
    ) -> Result<Client, CliError> {
        let stream = UnixStream::connect(sock_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                CliError::NotRunning(NOT_RUNNING.into())
            }
            _ => CliError::from(e),
        })?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut c = Client {
            stream,
            reader,
            next_id: 0,
            timeout: None,
        };
        c.set_timeout(Some(timeout));
        let mut params = json!({"role": role});
        if let Some(t) = token {
            params["token"] = json!(t);
        }
        c.call("auth.hello", Some(params))?;
        Ok(c)
    }

    /// The default: role `cli`, 10 seconds.
    pub fn open(sock_path: &Path) -> Result<Client, CliError> {
        Client::connect(sock_path, "cli", None, 10.0)
    }

    fn send(&mut self, method: &str, params: Value) -> Result<i64, CliError> {
        self.next_id += 1;
        let frame =
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        let mut bytes = ferb_py::json::dumps(&frame).into_bytes();
        bytes.push(b'\n');
        self.stream.write_all(&bytes).map_err(|e| match e.kind() {
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                CliError::NotRunning("FERB daemon went away".into())
            }
            _ => CliError::from(e),
        })?;
        Ok(self.next_id)
    }

    fn read(&mut self) -> Result<Value, CliError> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err(CliError::NotRunning(
                "FERB daemon closed the connection".into(),
            )),
            Ok(_) => serde_json::from_str(&line).map_err(|e| CliError::Ferb {
                kind: "JSONDecodeError".into(),
                message: e.to_string(),
            }),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(CliError::NotRunning(format!(
                    "FERB didn't answer within {:.0} s",
                    self.timeout.unwrap_or(0.0)
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                Err(CliError::NotRunning("FERB daemon went away".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn error(msg: &Value) -> CliError {
        CliError::Rpc {
            code: msg["error"]["code"].as_i64().unwrap_or(0),
            message: ferb_py::value::py_str(&msg["error"]["message"]),
        }
    }

    /// client.py:86: wakes a reader blocked on this connection.
    pub fn interrupt(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

impl Rpc for Client {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        let rid = self.send(method, params.unwrap_or_else(|| json!({})))?;
        loop {
            let msg = self.read()?;
            if msg.get("id") != Some(&json!(rid)) {
                continue;
            }
            if msg.get("error").is_some() {
                return Err(Client::error(&msg));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    fn set_timeout(&mut self, seconds: Option<f64>) {
        self.timeout = seconds;
        let d = seconds.map(|s| Duration::from_secs_f64(s.max(0.001)));
        let _ = self.stream.set_read_timeout(d);
        let _ = self.stream.set_write_timeout(d);
    }

    fn stream(
        &mut self,
        method: &str,
        params: Option<Value>,
        on_event: &mut dyn FnMut(Value) -> bool,
    ) -> Result<(), CliError> {
        self.set_timeout(None);
        let rid = self.send(method, params.unwrap_or_else(|| json!({})))?;
        let event = format!("{method}.event");
        loop {
            let msg = self.read()?;
            if msg.get("method").and_then(Value::as_str) == Some(event.as_str()) {
                if !on_event(msg.get("params").cloned().unwrap_or(Value::Null)) {
                    return Ok(());
                }
            } else if msg.get("id") == Some(&json!(rid)) {
                if msg.get("error").is_some() {
                    return Err(Client::error(&msg));
                }
                return Ok(());
            }
        }
    }
}
