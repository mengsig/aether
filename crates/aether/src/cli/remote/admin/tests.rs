use super::*;

fn device(id: &str, name: &str, extra: serde_json::Value) -> Device {
    let mut device = serde_json::json!({
        "id": id, "name": name, "role": "operator",
        "created_at": "2026-10-01T08:00:00Z",
    });
    for (key, value) in extra.as_object().expect("an object") {
        device[key] = value.clone();
    }
    serde_json::from_value(device).expect("a device")
}

const PHONE: &str = "dev_01M4PH1G605RXFBHJAHG90SGEV";
const TABLET: &str = "dev_01M4PH1G605RXFBHJAHG90SGEW";
const OLD: &str = "dev_01M4PH1G605RXFBHJAHG90SGEX";

fn status(extra: serde_json::Value) -> RemoteStatus {
    let mut status = serde_json::json!({
        "revision": 3,
        "settings": {"enabled": true, "pairing": true, "public_origins": [], "push": false},
        "address": "127.0.0.1:8377", "allow_remote": false,
        "devices": 2, "connections": 1, "push_previews": false,
    });
    for (key, value) in extra.as_object().expect("an object") {
        status[key] = value.clone();
    }
    serde_json::from_value(status).expect("a status")
}

#[test]
fn enabling_keeps_what_was_not_asked_to_change() {
    let stored = RemoteSettings {
        enabled: false,
        pairing: false,
        public_origins: vec!["https://old.example".into()],
        push: true,
    };
    let bare = Enable {
        origins: Vec::new(),
        pairing: true,
        push: None,
    };
    let asked = bare.over(stored.clone());
    assert!(asked.enabled && asked.pairing && asked.push);
    assert_eq!(asked.public_origins, stored.public_origins);

    let explicit = Enable {
        origins: vec!["https://new.example".into()],
        pairing: false,
        push: Some(false),
    };
    let asked = explicit.over(stored);
    assert!(asked.enabled && !asked.pairing && !asked.push);
    assert_eq!(asked.public_origins, ["https://new.example"]);
}

#[test]
fn status_says_where_the_listener_is_or_why_it_is_not() {
    let open = status(serde_json::json!({"listening": "127.0.0.1:8377"}));
    assert_eq!(
        describe(&open),
        "remote      enabled, listening on 127.0.0.1:8377\n\
         address     127.0.0.1:8377\n\
         pairing     allowed\n\
         push        off\n\
         origins     none: reachable from this machine only\n\
         devices     2 paired, 1 connected\n"
    );
    let failing = status(serde_json::json!({"error": "address in use"}));
    assert_eq!(listener(&failing), "enabled, not listening: address in use");
    let settings = serde_json::json!({
        "enabled": false, "pairing": false, "push": true,
        "public_origins": ["https://a.example", "https://b.example"],
    });
    let closed = status(serde_json::json!({"settings": settings, "push_previews": true}));
    let described = describe(&closed);
    assert!(
        described.starts_with("remote      disabled\n"),
        "{described}"
    );
    assert!(described.contains("pairing     closed\npush        on, with previews\n"));
    assert!(described.contains("origins     https://a.example\n            https://b.example\n"));
}

#[test]
fn the_device_table_shows_each_devices_standing() {
    let seen = serde_json::json!({"last_seen_at": "2026-10-11T22:23:55Z", "connections": 2});
    let revoked = serde_json::json!({"revoked_at": "2026-10-05T10:00:00Z", "role": "observer"});
    let long_name = "a".repeat(40);
    let devices = [
        device(PHONE, "Kim's phone", seen),
        device(TABLET, &long_name, serde_json::json!({"push": true})),
        device(OLD, "old", revoked),
    ];
    let table = table(&devices);
    let rows: Vec<&str> = table.lines().collect();
    assert_eq!(rows.len(), 4, "a header and a row each: {table}");
    assert!(rows[0].starts_with("id "), "{table}");
    let phone = format!(
        "{PHONE}  {:<25}  operator  2026-10-11 22:23:55 UTC  connected ×2",
        "Kim's phone"
    );
    assert_eq!(rows[1], phone);
    assert!(
        rows[2].contains(&format!("{}…  operator  never", "a".repeat(24))),
        "{table}"
    );
    assert!(rows[2].ends_with("paired · push"), "{table}");
    assert!(rows[3].ends_with("observer  never                    revoked 2026-10-05"));
}

#[test]
fn a_device_is_found_by_a_name_only_one_paired_device_has() {
    let revoked = serde_json::json!({"revoked_at": "2026-10-05T10:00:00Z"});
    let devices = [
        device(PHONE, "phone", serde_json::json!({})),
        device(TABLET, "tablet", serde_json::json!({})),
        device(OLD, "tablet", revoked),
    ];
    assert_eq!(
        resolve(&devices, "phone").expect("found").to_string(),
        PHONE
    );
    let unrevoked = resolve(&devices, "tablet").expect("the revoked namesake does not count");
    assert_eq!(unrevoked.to_string(), TABLET);
    let missing = resolve(&devices, "Phone").expect_err("names are exact");
    assert!(
        missing
            .to_string()
            .contains("no paired device is named `Phone`")
    );

    let twins = [
        device(PHONE, "phone", serde_json::json!({})),
        device(TABLET, "phone", serde_json::json!({})),
    ];
    let ambiguous = resolve(&twins, "phone")
        .expect_err("two of them")
        .to_string();
    assert!(ambiguous.contains("2 paired devices"), "{ambiguous}");
    assert!(
        ambiguous.contains(PHONE) && ambiguous.contains(TABLET),
        "{ambiguous}"
    );
}
