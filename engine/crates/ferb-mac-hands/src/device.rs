//! The Mac's state as `device.facts` reports it (T-1130): what the native layer read, each fact `None` (left out)
//! when it couldn't be read, never guessed. The reading itself is `native::device::read` (macOS only); tests build
//! a [`Reading`] by hand.

use serde_json::{json, Map, Value};

/// The battery as IOKit's power sources describe it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    /// 0-100.
    pub percent: i64,
    pub charging: bool,
    /// "AC Power" or "Battery Power", as macOS names the source.
    pub power_source: String,
}

/// The network the Mac is on now: the primary interface's kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    /// "wifi", "ethernet", "other" or "offline".
    pub kind: String,
    /// The primary interface ("en0"); "" offline.
    pub interface: String,
}

/// One active display, in points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Display {
    pub width: i64,
    pub height: i64,
    pub main: bool,
    /// The Mac's own screen (CGDisplayIsBuiltin); false: an external display.
    pub built_in: bool,
}

/// The Mac's hardware (read once: it doesn't change while FERB runs). Never a serial number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    /// The marketing name ("MacBook Air (13-inch, M4, 2025)", the device tree's product-name; Apple silicon only).
    pub name: Option<String>,
    /// "Apple M4" (machdep.cpu.brand_string).
    pub chip: Option<String>,
    pub memory_bytes: Option<u64>,
}

/// Everything read now; `None` (or empty) for a fact this Mac didn't give.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The Mac's computer name (System Settings → General → About → Name).
    pub name: Option<String>,
    /// The model identifier ("Mac15,3").
    pub model: Option<String>,
    pub macos: Option<String>,
    /// `Some(None)`: this Mac has no battery; `None`: unreadable.
    pub battery: Option<Option<Battery>>,
    pub low_power_mode: Option<bool>,
    pub network: Option<Network>,
    pub screen_locked: Option<bool>,
    pub displays: Vec<Display>,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    /// Seconds since the epoch.
    pub booted_at: Option<f64>,
    pub woke_at: Option<f64>,
    /// The default output device's name ("MacBook Pro Speakers", "AirPods Pro").
    pub audio_output: Option<String>,
    /// What kind of device it is ([`audio_kind`]): "speakers", "headphones", "built_in", "bluetooth", "usb",
    /// "display", "airplay" or "other".
    pub audio_kind: Option<String>,
    pub hardware: Hardware,
    /// Some app is playing through the default output device now.
    pub audio_playing: Option<bool>,
    /// "dark" or "light".
    pub appearance: Option<String>,
    pub locale: Option<String>,
    pub languages: Vec<String>,
}

impl Reading {
    /// The facts as the tool returns them, in a fixed order; what wasn't read is left out. `when` turns an epoch time
    /// into the text the tool shows (the daemon's local ISO time).
    pub fn facts(&self, when: &dyn Fn(f64) -> String) -> Map<String, Value> {
        let mut m = Map::new();
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                m.insert(k.into(), v);
            }
        };
        put("model", self.model.clone().map(Value::from));
        let h = &self.hardware;
        if h.name.is_some() || h.chip.is_some() || h.memory_bytes.is_some() {
            let mut hw = Map::new();
            if let Some(n) = &h.name {
                hw.insert("name".into(), json!(n));
            }
            if let Some(c) = &h.chip {
                hw.insert("chip".into(), json!(c));
            }
            if let Some(m) = h.memory_bytes {
                hw.insert("memory_gb".into(), json!(m / (1 << 30)));
            }
            put("hardware", Some(Value::Object(hw)));
        }
        put("macos", self.macos.clone().map(Value::from));
        put(
            "battery",
            self.battery.as_ref().map(|b| match b {
                None => json!("none (this Mac has no battery)"),
                Some(b) => json!({"percent": b.percent, "charging": b.charging, "power_source": b.power_source}),
            }),
        );
        put("low_power_mode", self.low_power_mode.map(Value::from));
        put(
            "network",
            self.network
                .as_ref()
                .map(|n| json!({"kind": n.kind, "interface": n.interface})),
        );
        put("screen_locked", self.screen_locked.map(Value::from));
        if !self.displays.is_empty() {
            put(
                "displays",
                Some(Value::Array(
                    self.displays
                        .iter()
                        .map(|d| json!({"width": d.width, "height": d.height, "main": d.main, "built_in": d.built_in}))
                        .collect(),
                )),
            );
            put(
                "external_displays",
                Some(json!(self.displays.iter().filter(|d| !d.built_in).count())),
            );
        }
        if let Some(free) = self.disk_free_bytes {
            let mut disk = json!({"free_gb": gb(free)});
            if let Some(total) = self.disk_total_bytes {
                disk["total_gb"] = json!(gb(total));
            }
            put("disk", Some(disk));
        }
        put("booted_at", self.booted_at.map(|t| Value::from(when(t))));
        put("woke_at", self.woke_at.map(|t| Value::from(when(t))));
        put("audio_output", self.audio_output.clone().map(Value::from));
        put(
            "audio_output_kind",
            self.audio_kind.clone().map(Value::from),
        );
        put("audio_playing", self.audio_playing.map(Value::from));
        put("appearance", self.appearance.clone().map(Value::from));
        put("locale", self.locale.clone().map(Value::from));
        if !self.languages.is_empty() {
            put("languages", Some(json!(self.languages)));
        }
        m
    }
}

/// Bytes as gigabytes (10^9, as Finder counts them), one decimal.
fn gb(bytes: u64) -> f64 {
    (bytes as f64 / 1e8).round() / 10.0
}

/// `pmset`-free reading of a power-source dictionary's numbers: percent of the maximum capacity.
pub fn percent(current: f64, max: f64) -> i64 {
    if max <= 0.0 {
        return current.clamp(0.0, 100.0).round() as i64;
    }
    (current * 100.0 / max).clamp(0.0, 100.0).round() as i64
}

/// The output device's kind from CoreAudio's transport type and, for the built-in device, its data source (four-char
/// codes as big-endian numbers): never its name, which may be personal.
pub fn audio_kind(transport: u32, source: Option<u32>) -> &'static str {
    match &transport.to_be_bytes() {
        b"bltn" => match source.map(u32::to_be_bytes).as_ref() {
            Some(b"ispk") => "speakers",
            Some(b"hdpn") => "headphones",
            _ => "built_in",
        },
        b"blue" | b"blea" => "bluetooth",
        b"usb " => "usb",
        b"hdmi" | b"dprt" | b"thun" => "display",
        b"airp" => "airplay",
        _ => "other",
    }
}

/// The primary interface's kind from SystemConfiguration's interface type.
pub fn network_kind(sc_type: &str) -> &'static str {
    match sc_type {
        "IEEE80211" => "wifi",
        "Ethernet" => "ethernet",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_facts_are_left_out_never_guessed() {
        assert!(Reading::default().facts(&|_| String::new()).is_empty());
        let r = Reading {
            battery: Some(None),
            displays: vec![Display {
                width: 1512,
                height: 982,
                main: true,
                built_in: true,
            }],
            disk_free_bytes: Some(123_456_789_000),
            booted_at: Some(1.0),
            languages: vec!["en-IN".into()],
            ..Reading::default()
        };
        let f = r.facts(&|t| format!("t{t}"));
        assert_eq!(
            Value::Object(f).to_string(),
            r#"{"battery":"none (this Mac has no battery)","displays":[{"width":1512,"height":982,"main":true,"built_in":true}],"external_displays":0,"disk":{"free_gb":123.5},"booted_at":"t1","languages":["en-IN"]}"#
        );
        assert_eq!(percent(50.0, 200.0), 25);
        assert_eq!(percent(87.0, 100.0), 87);
        assert_eq!(network_kind("IEEE80211"), "wifi");
        let code = |c: &[u8; 4]| u32::from_be_bytes(*c);
        assert_eq!(audio_kind(code(b"bltn"), Some(code(b"ispk"))), "speakers");
        assert_eq!(audio_kind(code(b"bltn"), Some(code(b"hdpn"))), "headphones");
        assert_eq!(audio_kind(code(b"blue"), None), "bluetooth");
        assert_eq!(audio_kind(code(b"virt"), None), "other");
    }
}
