//! The shared types write the JSON Python writes (rust/fixtures/core/types.json).

mod common;

use common::{compact, fixture, strs};
use ferb_core::types::*;
use ferb_core::when;
use serde::{de::DeserializeOwned, Serialize};

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
    name: &str,
    python: &str,
) {
    let value: T = serde_json::from_str(python).unwrap_or_else(|e| panic!("{name}: {e}\n{python}"));
    let written = serde_json::to_string(&value).unwrap();
    assert_eq!(written, compact(python), "{name}");
    assert_eq!(
        serde_json::from_str::<T>(&written).unwrap(),
        value,
        "{name}"
    );
}

macro_rules! check_types {
    ($all:expr, $seen:expr, $($name:ident),+ $(,)?) => {$(
        let instances = strs(&$all[stringify!($name)]);
        assert!(!instances.is_empty(), "no instances of {}", stringify!($name));
        for python in instances {
            round_trip::<$name>(stringify!($name), python);
        }
        $seen.push(stringify!($name));
    )+};
}

#[test]
fn every_type_writes_pythons_json_and_reads_it_back() {
    let fx = fixture("types.json");
    let all = &fx["asdict"];
    let mut seen: Vec<&str> = Vec::new();
    check_types!(
        all,
        seen,
        SecretRef,
        SecretSpec,
        Task,
        Outcome,
        ActionIntent,
        Span,
        TrailEvent,
        MemoryItem,
        Target,
        ReceivedBy,
        ActionFacts,
        ActionSpec,
        ModelLimits,
        RuntimeEvent,
        TerminalInput,
        TerminalGrant,
        TabRef,
        ElementRef,
        DriverFeatures,
        WaitCondition,
        ActionResult,
        FormSubmit,
        PageElement,
        PageSnapshot,
        ComputerAction,
        Identity,
        RiskLevel,
    );
    let mut in_fixture: Vec<&str> = all
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    in_fixture.sort_unstable();
    seen.sort_unstable();
    assert_eq!(seen, in_fixture, "a type in the fixture has no Rust check");
}

#[test]
fn an_unknown_field_or_enum_value_is_refused() {
    assert!(serde_json::from_str::<SecretRef>(r#"{"key": "k", "extra": 1}"#).is_err());
    assert!(serde_json::from_str::<Identity>(r#""someone""#).is_err());
    assert!(serde_json::from_str::<RiskLevel>("4").is_err());
    assert_eq!(Identity::ALL.len(), 2);
    assert_eq!(RuntimeEventKind::Compacted.as_str(), "compacted");
}

// decision: GATE-202
#[test]
fn recorded_facts_hold_only_what_is_known() {
    let fx = fixture("types.json");
    for case in fx["recorded"].as_array().unwrap() {
        let facts: ActionFacts = serde_json::from_str(case["facts"].as_str().unwrap()).unwrap();
        let recorded = serde_json::to_string(&facts.recorded()).unwrap();
        assert_eq!(recorded, compact(case["recorded"].as_str().unwrap()));
    }
    assert!(ActionFacts::default().recorded().is_empty());
}

#[test]
fn datetimes_are_written_as_python_writes_them() {
    let fx = fixture("types.json");
    for case in fx["datetimes"].as_array().unwrap() {
        let (iso, spaced) = (case["iso"].as_str().unwrap(), case["str"].as_str().unwrap());
        let dt = when::parse(iso).unwrap_or_else(|| panic!("{iso}"));
        assert_eq!(when::isoformat(&dt), iso);
        assert_eq!(when::py_str(&dt), spaced);
        assert_eq!(when::parse(spaced), Some(dt));
    }
    assert_eq!(
        when::parse("2026-03-04T09:30:15Z"),
        when::parse("2026-03-04T09:30:15+00:00")
    );
    for bad in [
        "2026-03-04T09:30:15",
        "2026-03-04",
        "2026-13-04T09:30:15+00:00",
        "2026-03-04T09:30:15.+00:00",
        "",
    ] {
        assert_eq!(when::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_model_image_and_request_match_python() {
    let fx = fixture("types.json");
    let image = &fx["image"];
    let made = ModelImage::new(
        hex::decode(image["hex"].as_str().unwrap()).unwrap(),
        image["ref"].as_str().unwrap(),
    );
    assert_eq!(made.mime, image["mime"].as_str().unwrap());
    assert_eq!(made.base64(), image["base64"].as_str().unwrap());
    assert_eq!(made.data_url(), image["data_url"].as_str().unwrap());
    for n in 0..5 {
        // every padding length
        let short = ModelImage::new(made.data[..n].to_vec(), "r");
        assert_eq!(short.base64().len(), n.div_ceil(3) * 4);
    }
    assert_eq!(ModelImage::new(b"ab".to_vec(), "r").base64(), "YWI=");
    assert_eq!(ModelImage::new(b"a".to_vec(), "r").base64(), "YQ==");
    let request = &fx["request"];
    let made = ModelRequest {
        system: request["system"].as_str().unwrap().to_string(),
        parts: strs(&request["parts"])
            .into_iter()
            .map(String::from)
            .collect(),
        cacheable: 2,
        images: Vec::new(),
    };
    assert_eq!(made.context(), request["context"].as_str().unwrap());
    for case in fx["limits"].as_array().unwrap() {
        let limits: ModelLimits = serde_json::from_str(case["limits"].as_str().unwrap()).unwrap();
        assert_eq!(
            limits.context_budget(),
            case["context_budget"].as_i64().unwrap()
        );
    }
}

/// Rule 7: picture bytes and a grant's secret never reach a log through `Debug` (Python: `repr=False`).
#[test]
fn debug_text_leaves_out_picture_bytes_and_secrets() {
    let fx = fixture("types.json");
    assert!(!fx["image"]["repr"].as_str().unwrap().contains("data"));
    assert!(!fx["escape_grant_repr"]
        .as_str()
        .unwrap()
        .contains("hidden-value"));
    let image = format!("{:?}", ModelImage::new(vec![7, 7, 7], "evidence:7"));
    assert!(image.contains("evidence:7") && !image.contains("[7") && !image.contains("data"));
    let grant = EscapeGrant {
        rule: "rule".into(),
        confirmation_id: "c-1".into(),
        command: "ls".into(),
        cwd: "/tmp/invented".into(),
        secret: Some("hidden-value".into()),
    };
    assert!(!format!("{grant:?}").contains("hidden-value"));
    let result = ToolResult {
        image: Some(vec![9; 64]),
        ..Default::default()
    };
    assert!(format!("{result:?}").contains("<64 bytes>"));
}

#[test]
fn origins_come_from_the_sanitize_port() {
    assert_eq!(ferb_core::origin::OWNER, "owner");
    assert!(ferb_core::origin::is_external(
        &ferb_core::origin::external("web", "example.com")
    ));
}
