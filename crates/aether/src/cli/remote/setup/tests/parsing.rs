use super::super::origin_of;
use super::super::platform::{Distro, Platform};
use super::super::tailscale::{PortUse, TailscaleState, parse_serve, parse_state};
use super::fakes::{NODE, TARGET, serve_json, tailscale_json};

#[test]
fn platforms_are_told_apart_by_kernel_and_os_release() {
    let arch = "NAME=\"Arch Linux\"\nID=arch\n";
    let mint = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
    let wsl = "Linux version 5.15.90.1-microsoft-standard-WSL2";
    assert_eq!(
        Platform::detect("linux", None, Some(arch)),
        Platform::Linux(Distro::Arch)
    );
    assert_eq!(
        Platform::detect("linux", None, Some(mint)),
        Platform::Linux(Distro::Debian)
    );
    assert_eq!(
        Platform::detect("linux", Some(wsl), Some(arch)),
        Platform::Wsl
    );
    assert_eq!(
        Platform::detect("linux", Some("Linux version 6.1"), None),
        Platform::Linux(Distro::Other)
    );
    assert_eq!(Platform::detect("macos", None, None), Platform::MacOs);
    assert_eq!(
        Platform::detect("freebsd", None, None),
        Platform::Other("freebsd".to_owned())
    );
}

#[test]
fn macos_also_looks_inside_the_app_bundle_and_wsl_for_the_windows_program() {
    assert!(
        Platform::MacOs
            .tailscale_programs()
            .iter()
            .any(|p| p.contains("Tailscale.app"))
    );
    assert!(
        Platform::Wsl
            .tailscale_programs()
            .contains(&"tailscale.exe")
    );
}

#[test]
fn a_running_tailnet_with_names_and_certificates_is_ready() {
    let state = parse_state(&tailscale_json("Running", true, true)).expect("parses");
    let TailscaleState::Ready(node) = state else {
        panic!("not ready: {state:?}");
    };
    assert_eq!(node.dns_name, NODE, "the trailing dot is gone");
}

#[test]
fn each_backend_state_maps_to_what_the_person_must_do() {
    let case = |state: &str, magic, certs| {
        parse_state(&tailscale_json(state, magic, certs)).expect("parses")
    };
    assert_eq!(case("Stopped", true, true), TailscaleState::Stopped);
    assert_eq!(
        case("NeedsLogin", true, true),
        TailscaleState::LoggedOut {
            login_url: Some("https://login.tailscale.com/a/abc123".to_owned())
        }
    );
    assert_eq!(
        case("NeedsMachineAuth", true, true),
        TailscaleState::NeedsApproval
    );
    assert_eq!(case("Starting", true, true), TailscaleState::Connecting);
    assert_eq!(case("Running", false, true), TailscaleState::NoMagicDns);
    assert_eq!(case("Running", true, false), TailscaleState::NoHttpsCerts);
    assert!(matches!(
        case("Mystery", true, true),
        TailscaleState::NotRunning { .. }
    ));
}

#[test]
fn a_certificate_for_another_name_is_not_https_for_this_machine() {
    let json = r#"{"BackendState":"Running","Self":{"DNSName":"box.t.ts.net."},"CertDomains":["other.t.ts.net"]}"#;
    assert_eq!(
        parse_state(json).expect("parses"),
        TailscaleState::NoHttpsCerts
    );
}

#[test]
fn output_that_is_not_json_is_an_error_not_a_state() {
    assert!(parse_state("Tailscale is stopped.").is_err());
}

#[test]
fn serve_status_tells_free_from_ours_from_taken() {
    let ours = parse_serve(&serve_json(443, TARGET, true)).expect("parses");
    assert_eq!(
        ours.port_use(NODE, 443, TARGET),
        PortUse::Ours { funnel: true }
    );
    assert_eq!(ours.port_use(NODE, 8443, TARGET), PortUse::Free);

    let other = parse_serve(&serve_json(443, "http://127.0.0.1:3000", false)).expect("parses");
    assert_eq!(
        other.port_use(NODE, 443, TARGET),
        PortUse::Taken("http://127.0.0.1:3000".to_owned())
    );

    let localhost = parse_serve(&serve_json(443, "http://localhost:8377/", false)).expect("parses");
    assert_eq!(
        localhost.port_use(NODE, 443, TARGET),
        PortUse::Ours { funnel: false }
    );
}

#[test]
fn nothing_served_is_empty_or_blank_output() {
    for text in ["{}", "", "  \n"] {
        let config = parse_serve(text).expect("parses");
        assert_eq!(config.port_use(NODE, 443, TARGET), PortUse::Free);
    }
}

#[test]
fn a_tcp_forward_on_a_port_takes_it() {
    let config = parse_serve(r#"{"TCP":{"8443":{"TCPForward":"127.0.0.1:22"}}}"#).expect("parses");
    assert!(matches!(
        config.port_use(NODE, 8443, TARGET),
        PortUse::Taken(_)
    ));
}

#[test]
fn a_proxy_url_is_an_origin_or_it_is_refused_with_the_fix() {
    assert_eq!(
        origin_of("https://Aether.Example.com").expect("ok"),
        "https://aether.example.com"
    );
    assert_eq!(
        origin_of("https://10.0.0.5:8443").expect("ok"),
        "https://10.0.0.5:8443"
    );
    assert_eq!(
        origin_of("https://aether.example.com:443").expect("ok"),
        "https://aether.example.com",
        "as a browser writes it, which is what the daemon accepts"
    );
    for bad in [
        "http://aether.example.com",
        "aether.example.com",
        "https://aether.example.com/",
        "https://aether.example.com/app",
        "https://user@aether.example.com",
        "https://aether.example.com:0",
        "https://aether.example.com:99999",
        "https://",
        "https://-bad-.example.com",
    ] {
        assert!(origin_of(bad).is_err(), "{bad} should be refused");
    }
    let slash = origin_of("https://aether.example.com/")
        .expect_err("refused")
        .to_string();
    assert!(slash.contains("https://aether.example.com"), "{slash}");
}

#[test]
fn a_foreground_session_holds_its_port() {
    let json = format!(
        r#"{{"Foreground":{{"session-1":{{"Web":{{"{NODE}:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:3000"}}}}}}}}}}}}}}"#
    );
    let config = parse_serve(&json).expect("parses");
    assert_eq!(
        config.port_use(NODE, 443, TARGET),
        PortUse::Taken("http://127.0.0.1:3000".to_owned())
    );
    assert_eq!(config.port_use(NODE, 8443, TARGET), PortUse::Free);
}

#[test]
fn a_foreground_tcp_forward_and_funnel_are_seen() {
    let json = format!(
        r#"{{"Foreground":{{"s":{{"TCP":{{"8443":{{"TCPForward":"127.0.0.1:22"}}}},"AllowFunnel":{{"{NODE}:443":true}},"Web":{{"{NODE}:443":{{"Handlers":{{"/":{{"Proxy":"{TARGET}"}}}}}}}}}}}}}}"#
    );
    let config = parse_serve(&json).expect("parses");
    assert_eq!(
        config.port_use(NODE, 443, TARGET),
        PortUse::Ours { funnel: true }
    );
    assert!(matches!(
        config.port_use(NODE, 8443, TARGET),
        PortUse::Taken(_)
    ));
}

#[test]
fn a_service_does_not_take_the_machines_own_ports() {
    let json = r#"{"Services":{"svc:docs":{"TCP":{"443":{"HTTPS":true}},"Web":{"docs.t.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9000"}}}},"Tun":false}}}"#;
    let config = parse_serve(json).expect("parses");
    assert_eq!(config.port_use(NODE, 443, TARGET), PortUse::Free);
    assert_eq!(
        config.port_use("docs.t.ts.net", 443, TARGET),
        PortUse::Taken("http://127.0.0.1:9000".to_owned())
    );
}

#[test]
fn a_port_with_only_other_paths_is_not_free() {
    let json =
        format!(r#"{{"Web":{{"{NODE}:443":{{"Handlers":{{"/docs":{{"Path":"/srv/docs"}}}}}}}}}}"#);
    let config = parse_serve(&json).expect("parses");
    assert!(matches!(
        config.port_use(NODE, 443, TARGET),
        PortUse::Taken(_)
    ));
}

#[test]
fn a_status_with_parts_it_cannot_read_is_never_reported_free() {
    let config = parse_serve(r#"{"Mystery":{"x":1}}"#).expect("parses");
    assert_eq!(
        config.port_use(NODE, 443, TARGET),
        PortUse::Unknown("`Mystery`".to_owned())
    );
    let known = parse_serve(&serve_json(443, TARGET, false)).expect("parses");
    assert_eq!(known.port_use(NODE, 8443, TARGET), PortUse::Free);
}
