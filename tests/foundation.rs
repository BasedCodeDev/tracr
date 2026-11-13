use std::{
    path::{Path, PathBuf},
    process::Command,
};
use tracr::{
    app,
    config::{self, Environment},
    domain::{ProviderId, SessionKey, SourceId},
    providers::{Cursor, SessionProvider, fixture::FixtureProvider},
};

fn environment(root: &Path) -> Environment {
    Environment {
        cwd: root.join("work"),
        home: Some(root.join("home")),
        codex_home: Some(PathBuf::from("env-home")),
        config_home: Some(root.join("config")),
        state_home: Some(root.join("state")),
        cache_home: Some(root.join("cache")),
    }
}

#[test]
fn configuration_precedence_paths_and_deduplication() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let file = temp.path().join("settings.toml");
    std::fs::write(&file, "codex_homes = ['a/../source', 'source', 'other']").unwrap();
    let resolved = config::resolve(&env, Some(&file), &[]).unwrap();
    assert_eq!(resolved.codex_sources.len(), 2);
    assert_eq!(resolved.codex_sources[0].home, temp.path().join("source"));
    let cli = config::resolve(&env, Some(&file), &[PathBuf::from("cli")]).unwrap();
    assert_eq!(cli.codex_sources.len(), 1);
    assert_eq!(cli.codex_sources[0].home, env.cwd.join("cli"));
    let defaults = config::resolve(&env, None, &[]).unwrap();
    assert_eq!(defaults.codex_sources[0].home, env.cwd.join("env-home"));
    let mut no_override = env.clone();
    no_override.codex_home = None;
    assert_eq!(
        config::resolve(&no_override, None, &[])
            .unwrap()
            .codex_sources[0]
            .home,
        env.home.as_ref().unwrap().join(".codex")
    );
    assert!(!resolved.state_dir.exists());
    assert!(!resolved.cache_dir.exists());
}

#[test]
fn explicit_missing_malformed_and_unknown_configuration_are_errors() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let file = temp.path().join("config.toml");
    assert!(config::resolve(&env, Some(&file), &[]).is_err());
    for text in ["invalid = [", "unknown = true", "codex_homes = ['']"] {
        std::fs::write(&file, text).unwrap();
        assert!(config::resolve(&env, Some(&file), &[]).is_err());
    }
}

#[test]
fn identities_are_stable_and_do_not_require_existing_paths() {
    let p = ProviderId("codex".into());
    let a = config::absolute_path(Path::new("x/../missing"), Path::new("/base")).unwrap();
    let b = config::absolute_path(Path::new("missing"), Path::new("/base")).unwrap();
    assert_eq!(config::source_id(&p, &a), config::source_id(&p, &b));
    assert_ne!(
        config::source_id(&p, &a),
        config::source_id(&ProviderId("other".into()), &a)
    );
    use std::os::unix::ffi::OsStringExt;
    let invalid = PathBuf::from(std::ffi::OsString::from_vec(b"/base/\xff".to_vec()));
    let replacement = PathBuf::from("/base/�");
    assert_ne!(
        config::source_id(&p, &invalid),
        config::source_id(&p, &replacement)
    );
}

fn catalog() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions/catalog.json")
}

#[tokio::test]
async fn fixtures_paginate_read_and_preserve_cross_source_identity() {
    let provider = FixtureProvider::open(&catalog()).unwrap();
    let first = provider.list(None).await.unwrap();
    assert_eq!(first.sessions.len(), 2);
    assert!(first.next.is_some());
    let result = app::collect(&provider).await;
    assert!(result.complete);
    assert_eq!(result.sessions.len(), 6);
    assert_eq!(
        result
            .sessions
            .iter()
            .filter(|s| s.key.native_id == "same-id")
            .count(),
        2
    );
    assert!(
        result
            .sessions
            .iter()
            .filter_map(|s| s.cwd.as_ref())
            .all(|p| p.is_absolute())
    );
    assert!(
        provider
            .read(&first.sessions[0].key)
            .await
            .unwrap()
            .is_some()
    );
    let missing = SessionKey {
        provider: ProviderId("codex".into()),
        source_id: SourceId("absent".into()),
        native_id: "same-id".into(),
    };
    assert!(provider.read(&missing).await.unwrap().is_none());
    assert!(
        provider
            .list(Some(Cursor("not-a-fixture-cursor".into())))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn partial_failures_keep_results_and_duplicates_prefer_newer_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let mut fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(catalog()).unwrap()).unwrap();
    let mut duplicate = fixture["sessions"][0].clone();
    duplicate["updated_at"] = 999.into();
    duplicate["title"] = "Updated title".into();
    fixture["sessions"]
        .as_array_mut()
        .unwrap()
        .insert(2, duplicate);
    fixture["fail_at_page"] = 2.into();
    let path = temp.path().join("partial.json");
    std::fs::write(&path, fixture.to_string()).unwrap();
    let provider = FixtureProvider::open(&path).unwrap();
    let result = app::collect(&provider).await;
    assert!(!result.complete);
    assert!(result.error.is_some());
    assert_eq!(result.sessions.len(), 3);
    assert!(result.sessions.iter().any(|s| s.title == "Updated title"));
}

#[tokio::test]
async fn visible_results_filter_archives_and_have_deterministic_order() {
    let provider = FixtureProvider::open(&catalog()).unwrap();
    let mut sessions = app::collect(&provider).await.sessions;
    app::visible_sessions(&mut sessions, false);
    assert_eq!(sessions.len(), 5);
    assert!(sessions.iter().all(|s| !s.archived));
    assert!(
        sessions
            .windows(2)
            .all(|p| p[0].updated_at >= p[1].updated_at)
    );
    let last = &sessions[sessions.len() - 2..];
    assert!(last[0].key < last[1].key);
    let original = sessions.clone();
    sessions.reverse();
    app::visible_sessions(&mut sessions, false);
    assert_eq!(sessions, original);
}

fn command(temp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tracr"));
    cmd.env("HOME", temp)
        .env("XDG_CONFIG_HOME", temp.join("config"))
        .env("XDG_STATE_HOME", temp.join("state"))
        .env("XDG_CACHE_HOME", temp.join("cache"))
        .env_remove("CODEX_HOME");
    cmd
}

#[test]
fn cli_help_version_fixture_json_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    for flag in ["--help", "--version"] {
        assert!(
            command(temp.path())
                .arg(flag)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let output = command(temp.path())
        .args(["list", "--fixture"])
        .arg(catalog())
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["sessions"].as_array().unwrap().len(), 5);
    assert_eq!(json["complete"], true);
    assert!(json["error"].is_null());
    let archived = command(temp.path())
        .args(["list", "--fixture"])
        .arg(catalog())
        .args(["--include-archived", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&archived.stdout).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    let live = command(temp.path())
        .args(["--codex-bin", "/missing-codex-executable", "list"])
        .output()
        .unwrap();
    assert!(!live.status.success());
    assert!(String::from_utf8_lossy(&live.stderr).contains("incomplete inventory"));
    for subcommand in ["list", "browse"] {
        for arguments in [
            vec!["--codex-home", "/source", subcommand, "--fixture"],
            vec![subcommand, "--codex-home", "/source", "--fixture"],
        ] {
            let conflict = command(temp.path())
                .args(arguments)
                .arg(catalog())
                .output()
                .unwrap();
            assert_eq!(conflict.status.code(), Some(2));
            assert!(String::from_utf8_lossy(&conflict.stderr).contains("--codex-home"));
        }
        let conflict = command(temp.path())
            .args([subcommand, "--fixture"])
            .arg(catalog())
            .args(["--codex-home", "/source"])
            .output()
            .unwrap();
        assert_eq!(conflict.status.code(), Some(2));
    }
    let missing = command(temp.path())
        .args(["list", "--fixture", "missing.json"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
}

#[tokio::test]
async fn repeated_cursor_stops_without_discarding_sessions() {
    struct Repeating(FixtureProvider);
    impl SessionProvider for Repeating {
        fn capabilities(&self) -> tracr::domain::ProviderCapabilities {
            self.0.capabilities()
        }
        async fn list(
            &self,
            _: Option<Cursor>,
        ) -> Result<tracr::providers::SessionPage, tracr::providers::ProviderError> {
            let mut page = self.0.list(None).await?;
            page.next = Some(Cursor("repeated".into()));
            Ok(page)
        }
        async fn read(
            &self,
            key: &SessionKey,
        ) -> Result<Option<tracr::domain::SessionSummary>, tracr::providers::ProviderError>
        {
            self.0.read(key).await
        }
    }
    let provider = Repeating(FixtureProvider::open(&catalog()).unwrap());
    let result = app::collect(&provider).await;
    assert!(!result.complete);
    assert_eq!(result.sessions.len(), 2);
    assert!(
        result
            .error
            .unwrap()
            .to_string()
            .contains("repeated pagination")
    );
}

#[test]
fn invalid_fixture_and_partial_cli_output_are_actionable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture.json");
    for text in [
        "not json",
        r#"{"page_size":0,"sessions":[]}"#,
        r#"{"unknown":true,"sessions":[]}"#,
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(FixtureProvider::open(&path).is_err());
    }
    let mut fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(catalog()).unwrap()).unwrap();
    fixture["fail_at_page"] = 1.into();
    std::fs::write(&path, fixture.to_string()).unwrap();
    let output = command(temp.path())
        .args(["list", "--fixture"])
        .arg(&path)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["complete"], false);
    assert_eq!(json["sessions"].as_array().unwrap().len(), 2);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("synthetic failure")
    );
}
