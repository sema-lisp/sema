//! RFC 8252 loopback redirect capture + browser launch.
//!
//! Bind a throwaway listener on `127.0.0.1:<ephemeral>` *before* opening the
//! browser, send the user to the authorization URL, and capture the `code` +
//! `state` the authorization server redirects back. Loopback (not a custom URI
//! scheme) is the right choice for a CLI: no OS registration, and the redirect
//! never leaves the machine, so plain HTTP is correct here.

use std::time::Duration;

use url::Url;

/// The page shown in the browser tab after the redirect is captured.
const DONE_PAGE: &str =
    "<!doctype html><html><body style=\"font-family:sans-serif\"><h3>Login complete</h3>\
     <p>You can close this tab and return to the terminal.</p></body></html>";
const FAIL_PAGE: &str =
    "<!doctype html><html><body style=\"font-family:sans-serif\"><h3>Login failed</h3>\
     <p>Return to the terminal for details.</p></body></html>";

/// Drives the user through the browser leg of the flow and returns the captured
/// authorization `code`. The real implementation opens the browser + runs the
/// loopback listener; tests substitute an opener that simulates the AS.
pub trait RedirectDriver {
    /// Reject a full interactive login before discovery or dynamic client
    /// registration. Drivers that permit interactive consent use the default.
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }

    /// The redirect URI the authorization request must use.
    fn redirect_uri(&self) -> String;
    /// Send the user to `authorize_url`, capture the redirect, verify `state`,
    /// and return the authorization `code`.
    fn drive(&self, authorize_url: &str, expected_state: &str) -> Result<String, String>;
}

/// A function that sends the user to an authorization URL (opens the system
/// browser in production; a test may substitute one that drives the redirect).
pub type BrowserOpener = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Open the system browser at `url`. Returns `Err` on a headless box (no
/// browser) so callers can fall back to a manual/device flow.
pub fn open_browser(url: &str) -> Result<(), String> {
    webbrowser::open(url).map_err(|e| format!("failed to open a browser: {e}"))
}

/// A bound loopback listener plus the ability to launch the browser. The
/// `opener` is injectable so tests can simulate the authorization server
/// redirecting to the loopback URL without a real browser.
pub struct LoopbackDriver {
    server: tiny_http::Server,
    port: u16,
    opener: BrowserOpener,
    timeout: Duration,
}

impl LoopbackDriver {
    /// Bind a loopback listener that opens the real system browser.
    pub fn new(timeout: Duration) -> Result<Self, String> {
        Self::with_opener(timeout, Box::new(open_browser))
    }

    /// Bind a loopback listener with a custom opener (tests inject one that
    /// drives the redirect directly).
    pub fn with_opener(timeout: Duration, opener: BrowserOpener) -> Result<Self, String> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| format!("failed to bind loopback listener: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "loopback listener has no IP address".to_string())?
            .port();
        Ok(Self {
            server,
            port,
            opener,
            timeout,
        })
    }

    fn wait_for_code(
        &self,
        expected_state: &str,
        opener_errors: &std::sync::mpsc::Receiver<String>,
    ) -> Result<String, String> {
        const OPENER_ERROR_POLL: Duration = Duration::from_millis(10);
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            if let Ok(error) = opener_errors.try_recv() {
                return Err(error);
            }
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| "timed out waiting for the OAuth redirect".to_string())?;
            let request = self
                .server
                .recv_timeout(remaining.min(OPENER_ERROR_POLL))
                .map_err(|e| format!("loopback listener error: {e}"))?;
            let Some(request) = request else {
                continue;
            };

            // Browsers hit a loopback listener with stray requests (favicon,
            // preconnect, "/"). Answer those 404 and keep waiting — only the
            // authorization-server redirect to /callback carries the code.
            let path = request.url().split('?').next().unwrap_or("");
            if path != "/callback" {
                let _ = request.respond(tiny_http::Response::empty(404));
                continue;
            }

            let result = parse_callback(request.url(), expected_state);
            let page = if result.is_ok() { DONE_PAGE } else { FAIL_PAGE };
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..])
                .expect("static header");
            let _ = request.respond(tiny_http::Response::from_string(page).with_header(header));
            return result;
        }
    }
}

impl RedirectDriver for LoopbackDriver {
    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    fn drive(&self, authorize_url: &str, expected_state: &str) -> Result<String, String> {
        // Open the browser and listen concurrently: the opener triggers the
        // redirect that `wait_for_code` receives, so they must run in parallel.
        std::thread::scope(|scope| {
            let url = authorize_url.to_string();
            let opener = &self.opener;
            let (send_error, receive_error) = std::sync::mpsc::sync_channel(1);
            scope.spawn(move || {
                if let Err(error) = opener(&url) {
                    let _ = send_error.send(error);
                }
            });
            self.wait_for_code(expected_state, &receive_error)
        })
    }
}

/// Parse the loopback callback request URL, validate `state`, and extract the
/// authorization `code` (mapping an `error` redirect to an `Err`).
fn parse_callback(request_url: &str, expected_state: &str) -> Result<String, String> {
    // `request_url` is a path+query like `/callback?code=…&state=…`; give it a
    // base so the URL parser can read the query.
    let url = Url::parse(&format!("http://127.0.0.1{request_url}"))
        .map_err(|e| format!("could not parse redirect URL: {e}"))?;
    let pairs: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();

    if let Some(err) = pairs.get("error") {
        let desc = pairs
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        return Err(format!(
            "authorization server returned error `{err}`{}",
            if desc.is_empty() {
                String::new()
            } else {
                format!(": {desc}")
            }
        ));
    }

    let state = pairs
        .get("state")
        .ok_or_else(|| "redirect is missing the state parameter".to_string())?;
    if state != expected_state {
        return Err("OAuth state mismatch (possible CSRF); aborting login".to_string());
    }
    pairs
        .get("code")
        .cloned()
        .ok_or_else(|| "redirect is missing the authorization code".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opener_failure_aborts_without_waiting_for_redirect_timeout() {
        let timeout = Duration::from_millis(250);
        let driver = LoopbackDriver::with_opener(
            timeout,
            Box::new(|_| Err("sandbox denied browser launch".to_string())),
        )
        .expect("bind loopback listener");

        let started = std::time::Instant::now();
        let error = driver
            .drive("https://example.com/authorize", "state")
            .expect_err("a failed opener must abort the redirect flow");

        assert_eq!(error, "sandbox denied browser launch");
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "opener failure waited for the redirect timeout: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn parses_code_when_state_matches() {
        let code = parse_callback("/callback?code=abc123&state=xyz", "xyz").unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn rejects_state_mismatch() {
        let err = parse_callback("/callback?code=abc&state=evil", "xyz").unwrap_err();
        assert!(err.contains("state mismatch"), "got: {err}");
    }

    #[test]
    fn surfaces_error_redirect() {
        let err = parse_callback(
            "/callback?error=access_denied&error_description=user%20said%20no",
            "xyz",
        )
        .unwrap_err();
        assert!(err.contains("access_denied"), "got: {err}");
        assert!(err.contains("user said no"), "got: {err}");
    }

    #[test]
    fn missing_code_is_error() {
        let err = parse_callback("/callback?state=xyz", "xyz").unwrap_err();
        assert!(err.contains("missing the authorization code"), "got: {err}");
    }
}
