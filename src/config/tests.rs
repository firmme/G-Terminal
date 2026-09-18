use super::profile::*;
use super::search::*;
use super::settings::*;
use super::ssh::*;
use super::*;

#[test]
fn old_config_migrates_and_default_label_is_host() {
    let s: Settings = serde_json::from_str(
        r#"{"profiles":[{"name":"","host":"10.0.0.8","user":"root","port":22,"identity":""}]}"#,
    )
    .unwrap();
    assert_eq!(s.profiles[0].label(), "10.0.0.8");
    assert!(s.profiles[0].group.is_empty());
    assert!(!s.copy_on_select);
    assert!(s.restore_tabs);
    assert!(s.serial_profiles.is_empty());
    // A config that predates the field still gets the default engine.
    assert_eq!(s.search_engine, DEFAULT_SEARCH_ENGINE);
}

#[test]
fn colors_round_trip_and_default_to_none() {
    let mut settings = Settings::default();
    // Nothing is coloured until it is picked.
    assert!(settings.profiles.is_empty());
    settings
        .group_colors
        .insert("prod".into(), TAG_COLORS[0].into());
    settings.profiles.push(RemoteProfile {
        group: "prod".into(),
        color: TAG_COLORS[2].into(),
        ..Default::default()
    });
    let json = serde_json::to_string(&settings).unwrap();
    let back = Settings::parse(json.as_bytes()).unwrap();
    assert_eq!(
        back.group_colors.get("prod").map(String::as_str),
        Some(TAG_COLORS[0])
    );
    assert_eq!(back.profiles[0].color, TAG_COLORS[2]);
    assert_eq!(RemoteProfile::default().color, "");
    assert_eq!(SerialProfile::default().color, "");
}

#[test]
fn search_urls_follow_the_engine_and_escape_the_query() {
    assert_eq!(
        search_url("google", "hello world"),
        "https://www.google.com/search?q=hello%20world"
    );
    assert_eq!(
        search_url("bing", "a&b"),
        "https://www.bing.com/search?q=a%26b"
    );
    assert_eq!(
        search_url("baidu", "中国"),
        "https://www.baidu.com/s?wd=%E4%B8%AD%E5%9B%BD"
    );
    // An unknown key still produces a working default lookup.
    assert_eq!(search_url("nope", "x"), "https://www.google.com/search?q=x");
    assert_eq!(search_engine_label("baidu"), "百度");
    assert_eq!(search_engine_label("nope"), "nope");
}

/// A tab layout this build cannot rebuild used to fail the whole parse,
/// which silently replaced the saved connections with the defaults.
#[test]
fn an_unreadable_workspace_keeps_the_saved_connections() {
    let settings = Settings::parse(
        br#"{
                "profiles":[{"name":"prod","host":"10.0.0.8","user":"root","port":22}],
                "serial_profiles":[{"name":"router","port":"COM3","baud":115200,"group":""}],
                "workspace":[{"sessions":[{"Unknown":{"x":1}}],"layout":{"Leaf":0},"focused":0}]
            }"#,
    )
    .unwrap();
    assert_eq!(settings.profiles.len(), 1);
    assert_eq!(settings.profiles[0].host, "10.0.0.8");
    assert_eq!(settings.serial_profiles.len(), 1);
    assert_eq!(settings.serial_profiles[0].port, "COM3");
    assert!(settings.workspace.is_empty());
}

#[test]
fn only_a_file_that_is_not_json_at_all_is_rejected() {
    assert!(Settings::parse(b"{ not json").is_err());
    // A field with the wrong type costs that field, not the file.
    let settings =
        Settings::parse(br#"{"font_size":"huge","profiles":[{"host":"10.0.0.8"}]}"#).unwrap();
    assert_eq!(settings.font_size, Settings::default().font_size);
    assert_eq!(settings.profiles.len(), 1);
}

#[test]
fn serial_profiles_default_to_auto_115200_and_validate() {
    let profile = SerialProfile::default();
    assert!(profile.auto());
    assert_eq!(profile.baud, DEFAULT_BAUD);
    assert_eq!(profile.label(), "auto");
    assert!(profile.validate().is_ok());

    let pinned = SerialProfile {
        name: "路由器".into(),
        port: " COM3 ".into(),
        ..Default::default()
    };
    assert!(!pinned.auto());
    assert_eq!(pinned.label(), "路由器");
    assert!(pinned.validate().is_ok());

    assert!(
        SerialProfile {
            port: "  ".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        SerialProfile {
            port: "COM 3".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        SerialProfile {
            baud: 0,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn ssh_config_imports_only_host_blocks_with_a_hostname() {
    let config = "\
# ~/.ssh/config
Host *
    ServerAliveInterval 30
Host web web-alias
    HostName web.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_web
    ProxyJump bastion
Host wildcard-*
    HostName nope
Host no-hostname
    User nobody
";
    let profiles = parse_ssh_config(config);
    // Only the two aliases of the block that names a host, never `*`.
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].name, "web");
    assert_eq!(profiles[0].host, "web.example.com");
    assert_eq!(profiles[0].user, "deploy");
    assert_eq!(profiles[0].port, 2222);
    assert_eq!(profiles[0].identity, expand_home("~/.ssh/id_web"));
    assert_eq!(profiles[1].name, "web-alias");
    assert_eq!(profiles[1].host, "web.example.com");
    // A global option before the first Host must not leak into the import.
    assert!(profiles.iter().all(|p| p.port != 30));
    let jump = profiles[0].jump.as_ref().unwrap();
    assert_eq!(jump.host, "bastion");
    assert_eq!(jump.port, 22);
}

#[test]
fn ssh_config_accepts_equals_quotes_and_comments() {
    let config = r#"
Host=quoted
  HostName="a#b.example.com" # trailing comment
  Port=2200
"#;
    let profiles = parse_ssh_config(config);
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "quoted");
    assert_eq!(profiles[0].host, "a#b.example.com");
    assert_eq!(profiles[0].port, 2200);
}

#[test]
fn proxy_jump_splits_user_host_port_in_any_combination() {
    let jump = parse_proxy_jump("root@jump.example.com:2022").unwrap();
    assert_eq!(jump.user, "root");
    assert_eq!(jump.host, "jump.example.com");
    assert_eq!(jump.port, 2022);
    let plain = parse_proxy_jump("bastion").unwrap();
    assert_eq!(plain.user, "");
    assert_eq!(plain.host, "bastion");
    assert_eq!(plain.port, 22);
    // Multiple hops: only the first is usable here.
    assert_eq!(parse_proxy_jump("a,b,c").unwrap().host, "a");
    assert!(parse_proxy_jump("  ").is_none());
}

#[test]
fn proxy_jump_aliases_resolve_to_the_imported_host() {
    let config = "\
Host bastion
    HostName jumphost.example.com
    User admin
    Port 2022
Host app
    HostName 10.0.0.5
    ProxyJump bastion
";
    let profiles = parse_ssh_config(config);
    let app = profiles.iter().find(|p| p.name == "app").unwrap();
    let jump = app.jump.as_ref().unwrap();
    assert_eq!(jump.host, "jumphost.example.com");
    assert_eq!(jump.user, "admin");
    assert_eq!(jump.port, 2022);
}

#[test]
fn serial_profiles_round_trip_through_settings_json() {
    let settings = Settings {
        serial_profiles: vec![SerialProfile {
            port: "COM9".into(),
            baud: 57600,
            group: "设备".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.serial_profiles.len(), 1);
    assert_eq!(back.serial_profiles[0].port, "COM9");
    assert_eq!(back.serial_profiles[0].baud, 57600);
    assert_eq!(back.serial_profiles[0].group, "设备");
}
