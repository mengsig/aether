//! How the remote listener is published, as `tailscale serve status --json` reports it: what
//! the mount lease and `configure_remote` read before mounted traffic may flow.

use std::path::PathBuf;
use std::time::Duration;

use aether_remote::mount::Publication;
use serde_json::{Map, Value};

/// Longest a status read may take; the lease gives up on it at the same age.
const READ_TIMEOUT: Duration = aether_remote::mount::LEASE_AGE;
/// Deepest nesting of serve scopes read: sessions and services inside the top level.
const MAX_SCOPE_DEPTH: usize = 4;
/// Where Tailscale's command line lives on macOS when it is not on `PATH`.
const MACOS_APP: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

/// The `tailscale` programs to ask, first found first.
#[derive(Debug, Clone)]
pub struct TailscaleStatus {
    programs: Vec<PathBuf>,
}

impl TailscaleStatus {
    pub fn system() -> Self {
        Self {
            programs: vec!["tailscale".into(), MACOS_APP.into()],
        }
    }

    #[cfg(test)]
    pub(crate) fn at(program: PathBuf) -> Self {
        Self {
            programs: vec![program],
        }
    }

    /// Whether anything on this machine is published through Funnel. No Tailscale at all is
    /// private: then nothing can be. A Tailscale that cannot answer is an error.
    pub async fn read(&self) -> Result<Publication, String> {
        assert!(!self.programs.is_empty(), "there is a program to ask");
        for program in &self.programs {
            match tokio::time::timeout(READ_TIMEOUT, serve_status(program)).await {
                Ok(Ok(Some(json))) => return parse(&json),
                Ok(Ok(None)) => continue,
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(format!(
                        "`{} serve status` did not answer within {}s",
                        program.display(),
                        READ_TIMEOUT.as_secs()
                    ));
                }
            }
        }
        Ok(Publication::Private)
    }
}

/// The output of `<program> serve status --json`; `None` when the program is not installed.
async fn serve_status(program: &PathBuf) -> Result<Option<String>, String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(["serve", "status", "--json"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let shown = program.display();
    let output = match command.output().await {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not run `{shown} serve status`: {error}")),
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{shown} serve status` failed: {}", said.trim()));
    }
    String::from_utf8(output.stdout)
        .map(Some)
        .map_err(|_| format!("`{shown} serve status` printed something that is not text"))
}

/// `tailscale serve status --json`: a Funnel anywhere in it, foreground sessions and services
/// included, is a Funnel. A part this reader does not know may hide one, so it is an error.
pub(crate) fn parse(json: &str) -> Result<Publication, String> {
    if json.trim().is_empty() {
        return Ok(Publication::Private);
    }
    let status: Value =
        serde_json::from_str(json).map_err(|e| format!("the serve status is not JSON: {e}"))?;
    let scope = status
        .as_object()
        .ok_or("the serve status is not a JSON object")?;
    match funnel_in(scope, 0)? {
        true => Ok(Publication::Funnel),
        false => Ok(Publication::Private),
    }
}

fn funnel_in(scope: &Map<String, Value>, depth: usize) -> Result<bool, String> {
    if depth > MAX_SCOPE_DEPTH {
        return Err("the serve status nests deeper than Tailscale writes it".into());
    }
    let mut funnel = false;
    for (key, value) in scope {
        funnel |= match key.as_str() {
            "TCP" | "Web" | "ETag" => false,
            "AllowFunnel" => any_allowed(value)?,
            "Foreground" | "Services" => any_scope_funnels(value, depth)?,
            other => {
                return Err(format!(
                    "the serve status has a part `{other}` that is not understood"
                ));
            }
        };
    }
    Ok(funnel)
}

/// `AllowFunnel`: `host:port` → whether Funnel is on. Anything but `false` counts as on.
fn any_allowed(value: &Value) -> Result<bool, String> {
    let ports = value.as_object().ok_or("`AllowFunnel` is not an object")?;
    Ok(ports.values().any(|on| on.as_bool() != Some(false)))
}

fn any_scope_funnels(value: &Value, depth: usize) -> Result<bool, String> {
    let scopes = value
        .as_object()
        .ok_or("a serve scope list is not an object")?;
    for scope in scopes.values() {
        let scope = scope.as_object().ok_or("a serve scope is not an object")?;
        if funnel_in(scope, depth + 1)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_configuration_or_serve_alone_is_private() {
        assert_eq!(parse(""), Ok(Publication::Private));
        assert_eq!(parse("{}"), Ok(Publication::Private));
        let serve = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"box.ts.net:443":{"Handlers":
            {"/":{"Proxy":"http://127.0.0.1:8377"}}}},"AllowFunnel":{"box.ts.net:443":false}}"#;
        assert_eq!(parse(serve), Ok(Publication::Private));
    }

    #[test]
    fn a_funnel_anywhere_is_a_funnel() {
        let top = r#"{"AllowFunnel":{"box.ts.net:443":true}}"#;
        let session = r#"{"Foreground":{"s1":{"AllowFunnel":{"box.ts.net:8443":true}}}}"#;
        let service = r#"{"Services":{"svc:x":{"AllowFunnel":{"x.ts.net:443":true}}}}"#;
        let odd = r#"{"AllowFunnel":{"box.ts.net:443":"yes"}}"#;
        for funnel in [top, session, service, odd] {
            assert_eq!(parse(funnel), Ok(Publication::Funnel), "{funnel}");
        }
    }

    #[test]
    fn a_status_that_cannot_be_read_whole_is_an_error() {
        for unreadable in [
            "not json",
            "[]",
            r#"{"Unknown":{}}"#,
            r#"{"AllowFunnel":[]}"#,
            r#"{"Foreground":{"s1":{"Mystery":1}}}"#,
            r#"{"Foreground":{"s1":3}}"#,
        ] {
            assert!(parse(unreadable).is_err(), "{unreadable}");
        }
        let deep = (0..6).fold(r#"{"AllowFunnel":{"a:1":true}}"#.to_owned(), |inner, _| {
            format!(r#"{{"Foreground":{{"s":{inner}}}}}"#)
        });
        assert!(parse(&deep).unwrap_err().contains("deeper"));
    }

    #[tokio::test]
    async fn no_tailscale_installed_is_private_and_a_failing_one_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = TailscaleStatus::at(dir.path().join("no-such-tailscale"));
        assert_eq!(missing.read().await, Ok(Publication::Private));
        let failing = TailscaleStatus::at("false".into());
        assert!(failing.read().await.unwrap_err().contains("failed"));
    }
}
