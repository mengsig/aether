//! Authentication delegated to an official CLI (Claude Code, Codex).
//!
//! The CLI owns the credential; Aether only asks it whether it is logged in
//! and hands the user to its login flow. Nothing is impersonated: inference
//! for these providers also runs through the CLI (see the engine crate).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::auth::types::AuthError;

/// Status and login commands of a credential-owning CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliAuth {
    /// Program name or path, resolved through `PATH`.
    pub program: String,
    /// Arguments that print login status and exit 0.
    pub status: Vec<String>,
    /// Arguments that run the interactive login flow.
    pub login: Vec<String>,
    #[serde(default)]
    pub logout: Vec<String>,
    /// Substrings in the status output that mean "logged in".
    pub logged_in_markers: Vec<String>,
    /// Shown in login pickers, e.g. "Claude subscription (Claude Code)".
    pub label: String,
    /// The CLI's OAuth credential file, `~` meaning the home directory. Its
    /// status reports a login that has expired as still logged in, so an
    /// access token past its expiry with no refresh token is not ready.
    #[serde(default)]
    pub credential_file: Option<String>,
    /// Environment variable naming a directory that replaces the one holding
    /// `credential_file` (`CLAUDE_CONFIG_DIR`), as it does for the CLI.
    #[serde(default)]
    pub credential_dir_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliStatus {
    /// The program exists and answered.
    pub available: bool,
    pub logged_in: bool,
    /// One line of detail for the UI.
    pub detail: String,
}

const CACHE_TTL: Duration = Duration::from_secs(30);
/// "Not logged in" is re-checked quickly so a login just completed shows up.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(3);

fn cache() -> &'static Mutex<HashMap<String, (Instant, CliStatus)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, CliStatus)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl CliAuth {
    fn cache_key(&self) -> String {
        format!("{} {}", self.program, self.status.join(" "))
    }

    /// Ask the CLI whether it is logged in. Cached briefly: callers such as
    /// model listings ask often and the CLI takes a moment to answer.
    pub async fn status(&self) -> CliStatus {
        let key = self.cache_key();
        if let Ok(cache) = cache().lock()
            && let Some((at, status)) = cache.get(&key)
        {
            let ttl = if status.logged_in {
                CACHE_TTL
            } else {
                NEGATIVE_CACHE_TTL
            };
            if at.elapsed() < ttl {
                return status.clone();
            }
        }
        let status = self.status_uncached().await;
        if let Ok(mut cache) = cache().lock() {
            cache.insert(key, (Instant::now(), status.clone()));
        }
        status
    }

    async fn status_uncached(&self) -> CliStatus {
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(&self.program)
                .args(&self.status)
                .stdin(std::process::Stdio::null())
                // A timeout or cancelled caller must not leave the probe running.
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match output {
            Ok(Ok(out)) => self.output_status(out),
            Ok(Err(e)) => CliStatus {
                available: false,
                logged_in: false,
                detail: if e.kind() == std::io::ErrorKind::NotFound {
                    format!("{} not found ({e})", self.program)
                } else {
                    format!("{} could not run ({e})", self.program)
                },
            },
            Err(_) => CliStatus {
                available: true,
                logged_in: false,
                detail: format!("{} status timed out", self.program),
            },
        }
    }

    fn output_status(&self, out: std::process::Output) -> CliStatus {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let mut logged_in =
            out.status.success() && self.logged_in_markers.iter().any(|m| text.contains(m));
        let expired = logged_in && self.login_expired();
        logged_in &= !expired;
        let reason = if !out.status.success() {
            format!("status returned non-zero exit ({})", out.status)
        } else if expired {
            format!("login expired, run `{} login`", self.program)
        } else if logged_in {
            "logged in".into()
        } else {
            "not logged in".into()
        };
        CliStatus {
            available: true,
            logged_in,
            detail: format!("{}: {reason}", self.program),
        }
    }

    fn login_expired(&self) -> bool {
        let override_dir = self
            .credential_dir_env
            .as_deref()
            .and_then(std::env::var_os)
            .filter(|dir| !dir.is_empty());
        self.login_expired_in(override_dir)
    }

    fn login_expired_in(&self, override_dir: Option<std::ffi::OsString>) -> bool {
        let Some(file) = self.credential_file.as_deref() else {
            return false;
        };
        let path = match (override_dir, std::path::Path::new(file).file_name()) {
            (Some(dir), Some(name)) => std::path::Path::new(&dir).join(name),
            _ => match expand_home(file) {
                Some(path) => path,
                None => return false,
            },
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        oauth_expired(&text, aether_core::time::now())
    }

    /// Run the CLI's own login flow attached to the current terminal.
    pub async fn login(&self) -> Result<(), AuthError> {
        self.run_inherit(&self.login).await?;
        self.forget();
        Ok(())
    }

    pub async fn logout(&self) -> Result<(), AuthError> {
        if self.logout.is_empty() {
            return Err(AuthError::Flow(format!(
                "{} has no logout command",
                self.program
            )));
        }
        self.run_inherit(&self.logout).await?;
        self.forget();
        Ok(())
    }

    async fn run_inherit(&self, args: &[String]) -> Result<(), AuthError> {
        let status = tokio::process::Command::new(&self.program)
            .args(args)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await
            .map_err(|e| AuthError::Flow(format!("could not run {}: {e}", self.program)))?;
        if !status.success() {
            return Err(AuthError::Flow(format!(
                "{} {} exited with {status}",
                self.program,
                args.join(" ")
            )));
        }
        Ok(())
    }

    /// Drop the cached status so the next check reflects a fresh login.
    pub fn forget(&self) {
        if let Ok(mut cache) = cache().lock() {
            cache.remove(&self.cache_key());
        }
    }
}

fn expand_home(path: &str) -> Option<std::path::PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest)),
        None => Some(path.into()),
    }
}

/// True when the credential file holds an access token past its expiry and no
/// refresh token to renew it. Unreadable or unfamiliar content is not expiry.
fn oauth_expired(text: &str, now: aether_core::time::Timestamp) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let oauth = &json["claudeAiOauth"];
    let Some(expires_ms) = oauth["expiresAt"].as_i64() else {
        return false;
    };
    let has_refresh = oauth["refreshToken"]
        .as_str()
        .is_some_and(|t| !t.is_empty());
    !has_refresh && expires_ms <= now.timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(script: &str) -> CliAuth {
        CliAuth {
            program: "sh".into(),
            status: vec!["-c".into(), script.into()],
            login: vec![],
            logout: vec![],
            logged_in_markers: vec!["\"loggedIn\": true".into(), "Logged in".into()],
            label: "fake".into(),
            credential_file: None,
            credential_dir_env: None,
        }
    }

    #[tokio::test]
    async fn expired_login_without_refresh_token_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        std::fs::write(
            &file,
            r#"{"claudeAiOauth":{"accessToken":"x","expiresAt":1000}}"#,
        )
        .unwrap();
        let auth = CliAuth {
            credential_file: Some(file.to_string_lossy().into_owned()),
            ..fake("printf '{\"loggedIn\": true}'")
        };
        let status = auth.status_uncached().await;
        assert!(!status.logged_in, "{status:?}");
        assert_eq!(status.detail, "sh: login expired, run `sh login`");
    }

    #[tokio::test]
    async fn config_dir_override_selects_the_credential_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.json"),
            r#"{"claudeAiOauth":{"expiresAt":1000}}"#,
        )
        .unwrap();
        let auth = CliAuth {
            credential_file: Some("/nonexistent/credentials.json".into()),
            ..fake("true")
        };
        assert!(auth.login_expired_in(Some(dir.path().into())));
        assert!(!auth.login_expired_in(None));
    }

    #[test]
    fn oauth_expiry_needs_a_past_expiry_and_no_refresh_token() {
        let now = aether_core::time::now();
        let past = now.timestamp_millis() - 1000;
        let future = now.timestamp_millis() + 60_000;
        let doc = |at: i64, refresh: &str| {
            format!(r#"{{"claudeAiOauth":{{"expiresAt":{at},"refreshToken":"{refresh}"}}}}"#)
        };
        assert!(oauth_expired(&doc(past, ""), now));
        assert!(!oauth_expired(&doc(past, "r"), now));
        assert!(!oauth_expired(&doc(future, ""), now));
        assert!(!oauth_expired("not json", now));
        assert!(!oauth_expired("{}", now));
    }

    #[tokio::test]
    async fn parses_logged_in_and_out() {
        let yes = fake("printf '{\"loggedIn\": true, \"email\": \"x\"}'");
        let s = yes.status().await;
        assert!(s.available && s.logged_in, "{s:?}");
        let no = fake("printf 'Not logged in'; exit 0");
        assert!(!no.status().await.logged_in);
        let failing = fake("printf 'Logged in'; exit 3");
        assert!(
            !failing.status().await.logged_in,
            "exit code must fail the check"
        );
    }

    #[tokio::test]
    async fn json_status_details_explain_outcome_instead_of_showing_a_brace() {
        let logged_out = fake("printf '{\n  \"loggedIn\": false\n}\n'")
            .status()
            .await;
        assert!(logged_out.available);
        assert!(!logged_out.logged_in);
        assert_eq!(logged_out.detail, "sh: not logged in");
        let failing = fake("printf '{\n  \"loggedIn\": true\n}\n'; exit 7")
            .status()
            .await;
        assert!(!failing.logged_in);
        assert!(failing.detail.contains("non-zero exit"), "{failing:?}");
        let ready = fake("printf '{\n  \"loggedIn\": true\n}\n'").status().await;
        assert!(ready.logged_in);
        assert_eq!(ready.detail, "sh: logged in");
    }

    #[tokio::test]
    async fn status_timeout_reports_the_program_and_reason() {
        let auth = fake("exec sleep 30");
        let status = auth.status_uncached().await;
        assert!(status.available);
        assert!(!status.logged_in);
        assert_eq!(status.detail, "sh status timed out");
    }

    #[tokio::test]
    async fn missing_program_is_unavailable() {
        let missing = CliAuth {
            program: "definitely-not-a-real-binary-aether".into(),
            ..fake("true")
        };
        let s = missing.status().await;
        assert!(!s.available && !s.logged_in);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelled_status_probe_does_not_outlive_its_caller() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("probe.pid");
        let auth = CliAuth {
            status: vec![
                "-c".into(),
                "echo $$ > \"$1\"; exec sleep 30".into(),
                "probe".into(),
                pid_file.to_string_lossy().into_owned(),
            ],
            ..fake("true")
        };
        let probe = tokio::spawn(async move { auth.status_uncached().await });
        let started = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        probe.abort();
        assert!(probe.await.unwrap_err().is_cancelled());
        let pid = started.expect("probe published its process id");
        let process = std::path::PathBuf::from(format!("/proc/{pid}"));
        let reaped = tokio::time::timeout(Duration::from_secs(5), async {
            while process.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if reaped.is_err() {
            // Clean up only this fixture's process before reporting the regression.
            let status = tokio::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .await
                .expect("kill surviving fixture");
            assert!(status.success(), "could not clean up fixture {pid}");
        }
        assert!(reaped.is_ok(), "cancelled CLI probe {pid} survived");
    }
}
