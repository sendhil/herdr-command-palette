#[test]
fn manifest_declares_open_action_and_popup() {
    let raw = std::fs::read_to_string("herdr-plugin.toml").expect("manifest exists");
    let value: toml::Value = toml::from_str(&raw).expect("manifest parses");
    assert_eq!(value["id"].as_str(), Some("herdr.command-palette"));
    assert_eq!(value["name"].as_str(), Some("Herdr Command Palette"));
    assert_eq!(value["version"].as_str(), Some("0.1.0"));
    assert_eq!(value["min_herdr_version"].as_str(), Some("0.7.4"));
    assert_eq!(
        value["description"].as_str(),
        Some("Search commands and plugin actions, and focus live workspaces, tabs, and agents.")
    );
    assert_eq!(
        value["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["macos"]
    );
    let build = &value["build"][0];
    assert_eq!(
        build["command"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["cargo", "build", "--release", "--locked"]
    );
    let action = &value["actions"][0];
    assert_eq!(action["id"].as_str(), Some("open"));
    assert_eq!(action["title"].as_str(), Some("Open command palette"));
    assert_eq!(
        action["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["global"]
    );
    assert_eq!(
        action["command"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["./target/release/herdr-command-palette", "open"]
    );
    let pane = &value["panes"][0];
    assert_eq!(pane["id"].as_str(), Some("palette"));
    assert_eq!(pane["title"].as_str(), Some("Command Palette"));
    assert_eq!(
        pane["command"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["./target/release/herdr-command-palette", "run"]
    );
    assert_eq!(pane["placement"].as_str(), Some("popup"));
    assert_eq!(pane["width"].as_str(), Some("80%"));
    assert_eq!(pane["height"].as_str(), Some("60%"));

    let cargo_raw = std::fs::read_to_string("Cargo.toml").expect("Cargo.toml exists");
    let cargo: toml::Value = toml::from_str(&cargo_raw).expect("Cargo.toml parses");
    let package = &cargo["package"];
    assert_eq!(
        package["description"].as_str(),
        Some("A macOS Herdr command palette for commands, plugin actions, and live workspaces")
    );
    assert_eq!(
        package["repository"].as_str(),
        Some("https://github.com/sendhil/herdr-command-palette")
    );
    assert_eq!(
        package["homepage"].as_str(),
        Some("https://github.com/sendhil/herdr-command-palette")
    );
    assert_eq!(package["rust-version"].as_str(), Some("1.88"));
    assert_eq!(
        package["keywords"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["herdr", "command-palette", "terminal", "macos"]
    );
    assert_eq!(
        package["categories"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["command-line-utilities"]
    );
    assert_eq!(
        package["exclude"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["docs/superpowers/**", ".pi-subagents/**"]
    );
}
