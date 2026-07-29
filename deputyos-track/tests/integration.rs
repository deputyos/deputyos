#![allow(clippy::unwrap_used)]
//! Integration tests for `deputyos-track`.
//!
//! We stub `api.github.com` with a `tiny_http` server bound to
//! `127.0.0.1:0` so the tests don't require network access. Each test
//! spins its own server, points a `github::Client` at it, and exercises
//! one path through the discovery + patch logic.

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use deputyos_track::github::{Channel, Client};
use deputyos_track::patch::bump_pinned_version;
use deputyos_track::profile;
use deputyos_track::version::Version;

/// Spawn a tiny HTTP server that responds to a single configured path
/// with a given status + body. Returns the bound address; the server
/// thread parks itself when it has served all expected requests.
fn spawn_stub(routes: Vec<(String, u16, String)>) -> SocketAddr {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
    let addr = server.server_addr().to_ip().expect("ip addr");
    let routes = Arc::new(routes);
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let matched = routes.iter().find(|(p, _, _)| url.starts_with(p));
            match matched {
                Some((_, status, body)) => {
                    let resp = tiny_http::Response::new(
                        tiny_http::StatusCode(*status),
                        vec![tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap()],
                        Cursor::new(body.clone().into_bytes()),
                        Some(body.len()),
                        None,
                    );
                    let _ = req.respond(resp);
                }
                None => {
                    let _ = req.respond(
                        tiny_http::Response::from_string("not found").with_status_code(404),
                    );
                }
            }
        }
    });
    // Give the server a moment to be ready.
    thread::sleep(Duration::from_millis(50));
    addr
}

fn release_json(tag: &str, prerelease: bool) -> String {
    serde_json::json!({
        "tag_name": tag,
        "name": format!("Release {tag}"),
        "published_at": "2026-04-26T00:00:00Z",
        "prerelease": prerelease,
        "draft": false,
        "body": "release notes here",
        "html_url": format!("https://example/{tag}")
    })
    .to_string()
}

#[test]
fn latest_stable_returns_release() {
    let body = release_json("v2026.4.27", false);
    let addr = spawn_stub(vec![(
        "/repos/openclaw/openclaw/releases/latest".into(),
        200,
        body,
    )]);
    let client = Client::with_base_url(format!("http://{addr}"));
    let r = client
        .latest("openclaw/openclaw", Channel::Stable)
        .expect("ok")
        .expect("some");
    assert_eq!(r.tag_name, "v2026.4.27");
    assert_eq!(r.html_url.as_deref(), Some("https://example/v2026.4.27"));
}

#[test]
fn latest_stable_404_yields_none() {
    let addr = spawn_stub(vec![(
        "/repos/foo/bar/releases/latest".into(),
        404,
        r#"{"message":"not found"}"#.into(),
    )]);
    let client = Client::with_base_url(format!("http://{addr}"));
    let r = client.latest("foo/bar", Channel::Stable).expect("ok");
    assert!(r.is_none());
}

#[test]
fn latest_beta_picks_first_non_draft() {
    let body = serde_json::json!([
        {"tag_name": "v2026.5.1-beta1", "prerelease": true, "draft": false, "body": "", "html_url": "u1", "published_at": "2026-04-30T00:00:00Z"},
        {"tag_name": "v2026.4.30", "prerelease": false, "draft": false, "body": "", "html_url": "u2", "published_at": "2026-04-29T00:00:00Z"}
    ])
    .to_string();
    let addr = spawn_stub(vec![("/repos/h/h/releases".into(), 200, body)]);
    let client = Client::with_base_url(format!("http://{addr}"));
    let r = client
        .latest("h/h", Channel::Beta)
        .expect("ok")
        .expect("some");
    assert_eq!(r.tag_name, "v2026.5.1-beta1");
}

#[test]
fn latest_handles_5xx_gracefully() {
    let addr = spawn_stub(vec![(
        "/repos/x/y/releases/latest".into(),
        503,
        r#"{"message":"down"}"#.into(),
    )]);
    let client = Client::with_base_url(format!("http://{addr}"));
    let res = client.latest("x/y", Channel::Stable);
    // 5xx surfaces as an Err — caller logs and continues.
    assert!(res.is_err());
}

#[test]
fn version_compare_cases() {
    assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2026.4.26").unwrap());
    assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2026.5.1").unwrap());
    assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2027.1.1").unwrap());
    assert!(Version::parse("2026.4.25-beta1").unwrap() < Version::parse("2026.4.25").unwrap());
    assert_eq!(
        Version::parse("v0.11.0").unwrap().parts,
        Version::parse("0.11.0").unwrap().parts
    );
}

#[test]
fn propose_produces_parseable_toml() {
    // Write a fake openclaw.toml to a tempdir, then exercise the patcher.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("openclaw.toml");
    let original = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("profiles/openclaw.toml"),
    )
    .expect("real openclaw.toml present in repo");
    std::fs::write(&path, &original).unwrap();

    let patch = bump_pinned_version(&original, "2026.4.27").unwrap();
    std::fs::write(&path, &patch.patched).unwrap();

    // The patched file must still parse and contain the new version.
    let profiles = profile::list(dir.path()).unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].profile.id, "openclaw");
    assert_eq!(profiles[0].profile.pinned_version, "2026.4.27");
}

#[test]
fn patch_preserves_other_lines_byte_for_byte() {
    let original = "[profile]\nid = \"x\"\npinned_version = \"1.0.0\"\n# tail\n";
    let patch = bump_pinned_version(original, "1.0.1").unwrap();
    assert!(patch.patched.starts_with("[profile]\nid = \"x\"\n"));
    assert!(patch.patched.ends_with("# tail\n"));
    assert!(patch.patched.contains("pinned_version = \"1.0.1\""));
}

#[test]
fn list_profiles_from_repo() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let profiles = profile::list(&repo_root.join("profiles")).unwrap();
    let ids: Vec<&str> = profiles.iter().map(|p| p.profile.id.as_str()).collect();
    assert!(ids.contains(&"openclaw"));
    assert!(ids.contains(&"hermes"));
}
