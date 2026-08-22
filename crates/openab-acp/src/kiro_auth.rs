//! Kiro CLI token reader with auto-refresh via AWS SSO OIDC.
//!
//! Reads the cached access token from kiro-cli's sqlite database. If expired,
//! uses the refresh_token + device registration to obtain a fresh token from
//! AWS SSO OIDC and persists it back.
//!
//! Unix-only: kiro-cli's `~/.local/share` data layout has no documented
//! Windows equivalent, so on non-Unix platforms the auth callback replies with
//! a JSON-RPC error instead (see `connection.rs`).
//!
//! The database location can be overridden with `KIRO_CLI_DATA_DIR` (a
//! directory containing `data.sqlite3`), which is required in containers
//! where the default `$HOME/.local/share/kiro-cli` path is not mounted.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error, info, warn};

const DB_RELATIVE_PATH: &str = ".local/share/kiro-cli/data.sqlite3";
const DB_FILE_NAME: &str = "data.sqlite3";
const TOKEN_KEY: &str = "kirocli:odic:token";
const REGISTRATION_KEY: &str = "kirocli:odic:device-registration";
/// Cooldown after a failed refresh to avoid hammering AWS SSO OIDC and
/// spawning `kiro-cli whoami` in a tight loop when the token is genuinely
/// dead. Reset to 0 on a successful refresh.
static LAST_REFRESH_FAIL_MS: AtomicU64 = AtomicU64::new(0);
const REFRESH_FAIL_COOLDOWN_MS: u64 = 30_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenData {
    access_token: String,
    expires_at: String,
    refresh_token: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    start_url: String,
    #[serde(default)]
    oauth_flow: String,
    #[serde(default)]
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Registration {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    region: String,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

/// Resolve the kiro-cli sqlite database path.
///
/// Precedence: `KIRO_CLI_DATA_DIR` (container-friendly override, expected to
/// hold `data.sqlite3` directly) → `$HOME/.local/share/kiro-cli/data.sqlite3`.
fn db_path() -> Option<String> {
    if let Ok(dir) = std::env::var("KIRO_CLI_DATA_DIR") {
        if !dir.is_empty() {
            return Some(format!("{}/{}", dir.trim_end_matches('/'), DB_FILE_NAME));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(format!("{}/{}", home, DB_RELATIVE_PATH))
}

/// Validate an AWS region string before it is interpolated into the OIDC
/// endpoint URL. The value comes from a locally-owned sqlite file, but a
/// corrupted/hostile file must not be able to steer requests at an arbitrary
/// host (`oidc.<anything>.amazonaws.com` is attacker-controllable DNS only if
/// the subdomain is).
fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Strip sub-second precision from an RFC 3339 timestamp while preserving the
/// original timezone designator (`Z`, `z`, or `±HH:MM`). Naively truncating at
/// `.` and appending `Z` would silently reinterpret `+08:00` timestamps as UTC.
fn strip_fractional_seconds(s: &str) -> Option<String> {
    let dot = s.find('.')?;
    // The timezone designator is the first non-digit after the dot.
    let tz = s[dot + 1..]
        .find(['Z', 'z', '+', '-'])
        .map(|i| dot + 1 + i)?;
    Some(format!("{}{}", &s[..dot], &s[tz..]))
}

fn parse_rfc3339(expires_at: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    // Prefer full RFC 3339 (including fractional seconds); fall back to
    // stripping sub-second precision if the original cannot be parsed.
    chrono::DateTime::parse_from_rfc3339(expires_at)
        .ok()
        .or_else(|| {
            strip_fractional_seconds(expires_at)
                .and_then(|clean| chrono::DateTime::parse_from_rfc3339(&clean).ok())
        })
}

fn is_expired(expires_at: &str) -> bool {
    match parse_rfc3339(expires_at) {
        Some(exp) => chrono::Utc::now() >= exp,
        None => true, // If we can't parse, assume expired
    }
}

fn expires_at_ms(expires_at: &str) -> Option<u64> {
    parse_rfc3339(expires_at).map(|dt| dt.timestamp_millis() as u64)
}

fn read_token_data(conn: &rusqlite::Connection) -> Option<TokenData> {
    let value: String = conn
        .query_row(
            "SELECT value FROM auth_kv WHERE key = ?1",
            [TOKEN_KEY],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&value).ok()
}

fn read_registration(conn: &rusqlite::Connection) -> Option<Registration> {
    let value: String = conn
        .query_row(
            "SELECT value FROM auth_kv WHERE key = ?1",
            [REGISTRATION_KEY],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&value).ok()
}

fn refresh_token(token_data: &TokenData, reg: &Registration) -> Option<TokenData> {
    let region = if valid_region(&reg.region) {
        reg.region.as_str()
    } else {
        if !reg.region.is_empty() {
            warn!(
                region = %reg.region,
                "kiro registration has malformed region — refusing OIDC refresh"
            );
            return None;
        }
        "us-east-1"
    };
    let url = format!("https://oidc.{}.amazonaws.com/token", region);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": reg.client_id,
            "client_secret": reg.client_secret,
            "refresh_token": token_data.refresh_token,
        }))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .ok()?;

    if !resp.status().is_success() {
        error!(status = %resp.status(), "SSO OIDC token refresh failed");
        return None;
    }

    let refresh_resp: RefreshResponse = resp.json().ok()?;
    let now = chrono::Utc::now();
    let new_expires = now + chrono::Duration::seconds(refresh_resp.expires_in as i64);

    Some(TokenData {
        access_token: refresh_resp.access_token,
        refresh_token: refresh_resp
            .refresh_token
            .unwrap_or_else(|| token_data.refresh_token.clone()),
        expires_at: new_expires.to_rfc3339(),
        region: token_data.region.clone(),
        start_url: token_data.start_url.clone(),
        oauth_flow: token_data.oauth_flow.clone(),
        scopes: token_data.scopes.clone(),
    })
}

fn save_token_data(conn: &rusqlite::Connection, token_data: &TokenData) {
    if let Ok(json) = serde_json::to_string(token_data) {
        let _ = conn.execute(
            "UPDATE auth_kv SET value = ?1 WHERE key = ?2",
            rusqlite::params![json, TOKEN_KEY],
        );
    }
}

/// Result of a successful token fetch, including profile ARN.
#[derive(Debug, Clone)]
pub struct KiroAuthResult {
    pub access_token: String,
    pub profile_arn: Option<String>,
    pub expires_at: String,
}

/// Build the JSON-RPC result for `_kiro/auth/getAccessToken`.
///
/// Returns `None` if no valid token can be obtained.
pub fn build_auth_response() -> Option<serde_json::Value> {
    let auth = get_access_token()?;
    let expires_at = expires_at_ms(&auth.expires_at).unwrap_or_else(|| now_ms() + 1_800_000);
    Some(serde_json::json!({
        "accessToken": auth.access_token,
        "expiresAt": expires_at,
        "profileArn": auth.profile_arn,
    }))
}

fn read_profile_arn(conn: &rusqlite::Connection) -> Option<String> {
    let raw: String = conn
        .query_row(
            "SELECT value FROM state WHERE key = ?1",
            ["api.codewhisperer.profile"],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    // Value may be a JSON string, a JSON object {"arn":"...", ...}, or a plain ARN.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
        match value {
            serde_json::Value::String(s) => return Some(s),
            serde_json::Value::Object(obj) => {
                if let Some(arn) = obj.get("arn").and_then(|v| v.as_str()) {
                    return Some(arn.to_owned());
                }
            }
            _ => {}
        }
    }
    Some(raw)
}

/// Get a valid access token (and profile ARN), refreshing if necessary.
///
/// This function performs blocking I/O (sqlite + HTTP).
/// Call from `tokio::task::spawn_blocking` in async contexts.
pub fn get_access_token() -> Option<KiroAuthResult> {
    let path = db_path()?;
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    let mut token_data = read_token_data(&conn)?;

    if is_expired(&token_data.expires_at) {
        // If a recent refresh attempt failed, skip the expensive OIDC HTTP
        // call and `kiro-cli whoami` subprocess for the cooldown window so a
        // dead token can't drive a retry storm.
        let now = now_ms();
        if now.saturating_sub(LAST_REFRESH_FAIL_MS.load(Ordering::Relaxed))
            < REFRESH_FAIL_COOLDOWN_MS
        {
            debug!("kiro token expired but refresh in cooldown — skipping");
            return None;
        }
        info!("kiro token expired, refreshing...");
        let reg = read_registration(&conn);

        let refreshed = reg.and_then(|r| refresh_token(&token_data, &r));

        match refreshed {
            Some(new_data) => {
                token_data = new_data;
                save_token_data(&conn, &token_data);
                LAST_REFRESH_FAIL_MS.store(0, Ordering::Relaxed);
                info!("kiro token refreshed via OIDC");
            }
            None => {
                // OIDC refresh failed (token rotated or expired).
                // Fallback: invoke kiro-cli whoami to trigger its internal refresh.
                info!("OIDC refresh failed, falling back to kiro-cli whoami");
                let profile_arn = read_profile_arn(&conn);
                drop(conn);
                let _ = std::process::Command::new("kiro-cli")
                    .arg("whoami")
                    .env(
                        "PATH",
                        format!(
                            "{}/.local/bin:/usr/local/bin:/usr/bin:/bin",
                            std::env::var("HOME").unwrap_or_default()
                        ),
                    )
                    .output();
                // Re-read the refreshed token from sqlite
                let conn2 = rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .ok()?;
                token_data = read_token_data(&conn2)?;
                if is_expired(&token_data.expires_at) {
                    LAST_REFRESH_FAIL_MS.store(now, Ordering::Relaxed);
                    error!("kiro-cli whoami fallback also failed");
                    return None;
                }
                LAST_REFRESH_FAIL_MS.store(0, Ordering::Relaxed);
                info!("kiro token refreshed via kiro-cli fallback");
                return Some(KiroAuthResult {
                    access_token: token_data.access_token,
                    profile_arn: read_profile_arn(&conn2).or(profile_arn),
                    expires_at: token_data.expires_at,
                });
            }
        }
    } else {
        debug!("kiro token still valid");
    }

    let profile_arn = read_profile_arn(&conn);
    Some(KiroAuthResult {
        access_token: token_data.access_token,
        profile_arn,
        expires_at: token_data.expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_validation_rejects_injection() {
        assert!(valid_region("us-east-1"));
        assert!(valid_region("ap-northeast-3"));
        assert!(!valid_region(""));
        assert!(!valid_region("us-east-1.evil.example"));
        assert!(!valid_region("US-EAST-1"));
        assert!(!valid_region("us east 1"));
        assert!(!valid_region("us-east-1/../../etc"));
        assert!(!valid_region(&"a".repeat(65)));
    }

    #[test]
    fn fractional_second_fallback_preserves_offset() {
        // Full precision parses directly.
        assert!(parse_rfc3339("2026-01-02T03:04:05.123456789Z").is_some());
        // Over-precise fractional seconds (chrono caps at nanoseconds) must
        // never have their offset reinterpreted as UTC: a +08:00 timestamp
        // must parse to the same instant as its UTC equivalent, whether
        // chrono parses it natively or via the strip-fraction fallback.
        let dt = parse_rfc3339("2026-01-02T03:04:05.99999999999+08:00")
            .expect("over-precise fractional seconds should parse");
        assert_eq!(dt.format("%:z").to_string(), "+08:00");
        assert_eq!(
            dt.timestamp(),
            parse_rfc3339("2026-01-01T19:04:05Z").unwrap().timestamp(),
            "+08:00 instant must equal its UTC equivalent"
        );
        // Garbage stays garbage.
        assert!(parse_rfc3339("not a date").is_none());
    }

    #[test]
    fn db_path_honors_env_override() {
        // KIRO_CLI_DATA_DIR takes precedence over the HOME-relative default.
        // (Env mutation is contained: this is the only openab-acp test that
        // touches these vars, and the values are restored before returning.)
        let old_dir = std::env::var("KIRO_CLI_DATA_DIR").ok();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("KIRO_CLI_DATA_DIR", "/opt/kiro-data/");
        std::env::set_var("HOME", "/home/nobody");
        assert_eq!(db_path().as_deref(), Some("/opt/kiro-data/data.sqlite3"));
        std::env::remove_var("KIRO_CLI_DATA_DIR");
        assert_eq!(
            db_path().as_deref(),
            Some("/home/nobody/.local/share/kiro-cli/data.sqlite3")
        );
        std::env::remove_var("HOME");
        assert_eq!(db_path(), None);
        // Restore.
        if let Some(v) = old_dir {
            std::env::set_var("KIRO_CLI_DATA_DIR", v);
        }
        if let Some(v) = old_home {
            std::env::set_var("HOME", v);
        }
    }
}
