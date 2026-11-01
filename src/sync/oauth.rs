//! One-click Google sign-in.
//!
//! ## Why this exists next to `gdrive`'s device flow
//!
//! The device flow works, but it asks the user to read a code off the app,
//! switch to a browser, type the code, and approve. That is three steps and a
//! transcription error waiting to happen. Modern desktop apps do not do that:
//! the user presses **Sign in with Google**, a browser window opens already on
//! *their* account chooser, they click their name, and the app is signed in.
//!
//! That is the OAuth 2.0 Authorization Code flow with PKCE (RFC 7636), using a
//! loopback redirect. It is the flow Google documents for desktop apps, and it
//! needs no client secret to be kept — the binary cannot hold one safely.
//!
//! ## Which client type this requires
//!
//! A **Desktop app** OAuth client, not "TVs and Limited Input devices". Google
//! refuses a loopback redirect on the latter (verified: the error is
//! `Localhost URI is not allowed for 'NATIVE_DEVICE' client type`). It also
//! refuses the `drive.appdata` scope on the device flow entirely, so the
//! desktop client is the only shape that gives a one-click sign-in *and* the
//! hidden app folder.
//!
//! ## Why the state check and the PKCE verifier both matter
//!
//! The loopback listener is reachable by any process on the machine. Without a
//! `state` check, another local program could hand the app an authorization
//! code it did not ask for. Without PKCE, anyone who intercepted the redirect
//! could redeem the code. Neither is optional, so neither is skipped here.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use crate::sync::gdrive::Tokens;

/// Google's authorization and token endpoints.
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// The narrowest scope pair that can do the job: a hidden per-app folder in the
/// user's own Drive, plus the email so the card can say *which* account.
pub const SCOPES: &str =
    "https://www.googleapis.com/auth/drive.appdata https://www.googleapis.com/auth/userinfo.email";

/// How long to wait for the user to finish in the browser.
///
/// Generous, because the browser may need a password, a 2FA prompt, and an
/// account chooser. This is a deadline on *waiting for a human*, not on a
/// network call, so it should be long enough that it never fires in practice.
const WAIT_LIMIT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// A base64url string with no padding, which is what RFC 7636 requires for
/// both the verifier and its challenge.
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 32 bytes of randomness, base64url encoded. That is 43 characters, the
/// shortest length RFC 7636 allows for a verifier.
fn random_verifier() -> String {
    let mut buf = [0u8; 32];
    // `getrandom` is already in the tree via the TLS stack, so this adds no
    // dependency and, importantly, cannot silently fall back to a weak seed.
    getrandom::getrandom(&mut buf).expect("the OS random source is unavailable");
    b64url(&buf)
}

/// The S256 challenge: base64url(sha256(verifier)).
fn challenge_for(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// An opaque value that ties the redirect back to this request.
fn random_state() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("the OS random source is unavailable");
    b64url(&buf)
}

/// The redirect URI for a listener on `port`.
///
/// Google's desktop clients accept `http://127.0.0.1:<port>` on any port, so
/// the port does not have to be pre-registered. The path is fixed at `/` so
/// the request line is trivially parsed.
fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// Build the URL the browser should open.
pub fn authorize_url(client_id: &str, port: u16, state: &str, challenge: &str) -> String {
    format!(
        "{AUTH_URL}?client_id={cid}&redirect_uri={ru}&response_type=code\
         &scope={scope}&code_challenge={cc}&code_challenge_method=S256\
         &state={st}&access_type=offline&prompt=consent",
        cid = urlencode(client_id),
        ru = urlencode(&redirect_uri(port)),
        scope = urlencode(SCOPES),
        cc = urlencode(challenge),
        st = urlencode(state),
    )
}

/// Percent-encode the characters that actually appear in these values.
///
/// A full encoder is not worth a dependency here: the only inputs are a client
/// id, a scope list, and base64url strings. Colons, slashes, spaces and `+`
/// are the ones that break a query string, so those are what this handles.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The loopback listener
// ---------------------------------------------------------------------------

/// What the browser eventually sends back to the loopback listener.
#[derive(Debug, PartialEq)]
pub enum Callback {
    /// The user approved. This is the authorization code.
    Code(String),
    /// The user refused, or Google returned an error.
    Error(String),
}

/// The page shown in the browser after the redirect.
///
/// It has to say the window can be closed, because otherwise the user is
/// staring at a blank tab wondering whether it worked. It is deliberately
/// plain: no script, no external assets, nothing that can fail to load.
fn done_page(ok: bool, lang_en: bool) -> String {
    let (title, body, accent) = if ok {
        (
            if lang_en {
                "Pangan is connected"
            } else {
                "پنگان متصل شد"
            },
            if lang_en {
                "You can close this tab and go back to Pangan. Your focus history and settings now sync on their own."
            } else {
                "این پنجره را ببندید و به پنگان بازگردید. تاریخچه و تنظیمات شما از این پس خودکار همگام می‌شود."
            },
            "#c9a227",
        )
    } else {
        (
            if lang_en {
                "Sign-in did not finish"
            } else {
                "ورود کامل نشد"
            },
            if lang_en {
                "Nothing was changed. Close this tab and try again from Pangan."
            } else {
                "هیچ تغییری اعمال نشد. این پنجره را ببندید و از پنگان دوباره تلاش کنید."
            },
            "#a33",
        )
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{margin:0;min-height:100vh;display:flex;align-items:center;\
         justify-content:center;background:#12142a;color:#ece6d4;\
         font-family:system-ui,-apple-system,'Segoe UI',sans-serif}}\
         .c{{max-width:26rem;padding:2.5rem;text-align:center}}\
         .m{{width:3.5rem;height:3.5rem;border-radius:50%;margin:0 auto 1.5rem;\
         display:flex;align-items:center;justify-content:center;\
         background:{accent}22;border:2px solid {accent};font-size:1.6rem;\
         color:{accent}}}h1{{font-size:1.15rem;margin:0 0 .75rem;font-weight:600}}\
         p{{margin:0;line-height:1.7;color:#a9a494;font-size:.92rem}}</style>\
         </head><body><div class=\"c\"><div class=\"m\">{mark}</div>\
         <h1>{title}</h1><p>{body}</p></div></body></html>",
        mark = if ok { "&#10003;" } else { "!" },
    )
}

/// Pull `code`/`error` and `state` out of a request target like
/// `/ ?code=...&state=...`.
///
/// Split out from the socket loop so the parsing can be tested without a
/// browser: this is where a mis-parse would silently accept a foreign request.
pub fn parse_callback(target: &str) -> Option<(Callback, String)> {
    let query = target.split_once('?')?.1;
    let mut code = None;
    let mut state = String::new();
    let mut error = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        let v = urldecode(v);
        match k {
            "code" => code = Some(v),
            "state" => state = v,
            "error" => error = Some(v),
            _ => {}
        }
    }
    let outcome = match (code, error) {
        (Some(c), _) if !c.is_empty() => Callback::Code(c),
        (_, Some(e)) => Callback::Error(e),
        _ => return None,
    };
    Some((outcome, state))
}

/// Reverse the percent-encoding, including `+` as a space.
fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Read one HTTP request from the socket, answer it, and return what it asked
/// for.
///
/// Only the request line matters. Browsers send a `GET /?code=...` with no
/// body, so the headers are read and discarded purely so the connection closes
/// cleanly *after* the response is written — closing early is what makes some
/// browsers show a connection error instead of the page.
fn serve_one(listener: &TcpListener, lang_en: bool) -> Option<(Callback, String)> {
    let (mut stream, _) = listener.accept().ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    // Drain the headers so the browser sees a normal end of request.
    loop {
        let mut h = String::new();
        match reader.read_line(&mut h) {
            Ok(0) => break,
            Ok(_) if h == "\r\n" || h == "\n" => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let parsed = parse_callback(target);

    // A favicon request arrives on its own connection; answer it and keep
    // waiting rather than treating it as the callback.
    if target.starts_with("/favicon.ico") {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
        return None;
    }

    let body = done_page(
        parsed
            .as_ref()
            .is_some_and(|(c, _)| matches!(c, Callback::Code(_))),
        lang_en,
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
    parsed
}

// ---------------------------------------------------------------------------
// The whole flow
// ---------------------------------------------------------------------------

/// What a completed sign-in produced, plus how long it took, for the status
/// line.
#[allow(dead_code)] // `seconds` is elapsed time; the UI shows the outcome only
pub struct SignedIn {
    pub tokens: Tokens,
    pub seconds: u64,
}

/// Whether a `Location` header from the authorize endpoint means Google refused
/// the loopback redirect.
///
/// Split out as a pure function because the decision, not the request, is the
/// part that can be wrong: Google answers a good client with a redirect to its
/// account chooser and a bad one with a redirect to its own error page. The
/// substrings are stable across both, where the full URLs are long and tokenised.
pub fn location_means_blocked(location: &str) -> bool {
    location.contains("signin/oauth/error") || location.contains("authError=")
}

/// Ask Google whether this client id can use the loopback redirect.
///
/// A **Desktop app** client can; a **TVs and Limited Input devices** client
/// cannot, and Google answers with a redirect to its error page instead of to
/// the loopback address. That failure is invisible to the app — the browser
/// shows the error and the listener waits for a callback that will never
/// arrive — so it has to be detected *before* the browser opens.
///
pub fn supports_loopback(client_id: &str) -> bool {
    let url = authorize_url(client_id, 53682, "probe", &challenge_for("probe"));
    let resp = crate::sync::gdrive::http_agent()
        .get(&url)
        .config()
        // Following the redirect would land on Google's page and throw away
        // the header that carries the answer, turning this check into a
        // pointless round trip that always says "fine".
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .call();
    let Ok(r) = resp else {
        // Inconclusive, which in practice means the network is down. Report
        // "no" so the caller takes the device flow: it fails with its own
        // clear network error in seconds, where the loopback flow would sit
        // waiting five minutes for a callback that nothing is going to send.
        return false;
    };
    let location = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // No redirect at all means no verdict; assume the better flow is not
    // available rather than hanging on it.
    if location.is_empty() {
        return false;
    }
    !location_means_blocked(&location)
}

/// Run the full one-click flow: open the browser, wait for the redirect,
/// redeem the code.
///
/// `open` is injected rather than called directly so the flow can be exercised
/// in tests without launching anything, and so the caller decides how a URL
/// reaches the desktop's browser on each platform.
pub fn sign_in<F>(client_id: &str, lang_en: bool, open: F) -> Result<SignedIn, String>
where
    F: FnOnce(&str),
{
    let started = Instant::now();
    // Port 0 lets the OS pick a free port. Google accepts any port on a
    // desktop client, so there is no need to pin one and risk a collision.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not open a local sign-in port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let verifier = random_verifier();
    let challenge = challenge_for(&verifier);
    let state = random_state();
    let url = authorize_url(client_id, port, &state, &challenge);

    open(&url);

    listener
        .set_nonblocking(false)
        .map_err(|e| format!("listener setup failed: {e}"))?;

    let deadline = Instant::now() + WAIT_LIMIT;
    let (callback, got_state) = loop {
        if Instant::now() > deadline {
            return Err("The sign-in window timed out. Try again.".into());
        }
        match serve_one(&listener, lang_en) {
            Some(v) => break v,
            None => continue,
        }
    };

    // A mismatched state means this redirect is not the one this process
    // asked for. Refusing is the entire point of sending one.
    if got_state != state {
        return Err("sign-in was interrupted: the reply did not match the request".into());
    }

    match callback {
        Callback::Error(e) if e == "access_denied" => Err("sign-in was cancelled".into()),
        Callback::Error(e) => Err(format!("Google returned: {e}")),
        Callback::Code(code) => {
            let tokens = exchange(client_id, &code, &verifier, port)?;
            Ok(SignedIn {
                tokens,
                seconds: started.elapsed().as_secs(),
            })
        }
    }
}

/// Trade the authorization code for tokens.
///
/// The verifier goes along with it; Google recomputes the challenge and
/// refuses the exchange if it does not match, which is what makes PKCE work
/// without a client secret.
fn exchange(client_id: &str, code: &str, verifier: &str, port: u16) -> Result<Tokens, String> {
    // PKCE replaces the client secret for a Desktop-app client, so this is
    // normally empty and correctly omitted. It is sent when configured because
    // some client types still demand it, and sending an empty string would be
    // worse than sending nothing.
    let secret = crate::sync::gdrive::client_secret();
    let secret_part = if secret.is_empty() {
        String::new()
    } else {
        format!("&client_secret={}", urlencode(&secret))
    };
    let body = format!(
        "client_id={}&code={}&code_verifier={}&grant_type=authorization_code&redirect_uri={}{}",
        urlencode(client_id),
        urlencode(code),
        urlencode(verifier),
        urlencode(&redirect_uri(port)),
        secret_part,
    );
    let resp = crate::sync::gdrive::http_agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("token exchange failed: {e}")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("token response was not JSON: {e}"))?;
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        let desc = v
            .get("error_description")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        return Err(format!("{err}: {desc}"));
    }
    let access = v["access_token"].as_str().unwrap_or("");
    if access.is_empty() {
        return Err("no access token in the response".into());
    }
    Ok(Tokens {
        access_token: access.to_string(),
        refresh_token: v["refresh_token"].as_str().unwrap_or("").to_string(),
        expires_at: chrono::Utc::now().timestamp() + v["expires_in"].as_i64().unwrap_or(3600),
        email: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_base64url_of_the_sha256() {
        // RFC 7636's own worked example, so this is checked against the spec
        // rather than against whatever this code happens to produce.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn two_verifiers_are_never_the_same() {
        let a = random_verifier();
        let b = random_verifier();
        assert_ne!(a, b);
        // 32 bytes base64url encoded is 43 chars, the RFC minimum.
        assert_eq!(a.len(), 43);
    }

    #[test]
    fn the_authorize_url_carries_every_required_parameter() {
        let url = authorize_url("1234-abc.apps.googleusercontent.com", 53682, "st", "ch");
        for needle in [
            "response_type=code",
            "code_challenge_method=S256",
            "code_challenge=ch",
            "state=st",
            "access_type=offline",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2F",
        ] {
            assert!(url.contains(needle), "missing {needle} in {url}");
        }
        // The scope separator must be encoded, or the whole list is sent as
        // one malformed scope and Google rejects the request.
        assert!(
            url.contains("drive.appdata%20https%3A%2F%2Fwww.googleapis.com"),
            "the two scopes must be split by an encoded space, not a raw one"
        );
        assert!(
            !url.contains("scope=https://www.googleapis.com/auth/drive.appdata https"),
            "a raw space would truncate the scope list at the query string"
        );
        // The state must be its own parameter, not glued to the one before it.
        assert!(
            url.contains("&state=st&") || url.ends_with("&state=st"),
            "got {url}"
        );
    }

    #[test]
    fn a_code_callback_is_read_with_its_state() {
        let (cb, state) = parse_callback("/?code=4%2F0Abc&state=xyz").unwrap();
        assert_eq!(cb, Callback::Code("4/0Abc".into()));
        assert_eq!(state, "xyz");
    }

    #[test]
    fn a_refusal_is_read_as_an_error_not_as_a_code() {
        let (cb, _) = parse_callback("/?error=access_denied&state=s").unwrap();
        assert_eq!(cb, Callback::Error("access_denied".into()));
    }

    #[test]
    fn a_request_with_no_query_is_not_a_callback() {
        assert!(parse_callback("/").is_none());
        assert!(parse_callback("/?state=only").is_none());
        assert!(
            parse_callback("/?code=&state=s").is_none(),
            "an empty code is not a code"
        );
    }

    #[test]
    fn percent_decoding_handles_the_characters_google_uses() {
        assert_eq!(urldecode("4%2F0Abc"), "4/0Abc");
        assert_eq!(urldecode("a+b"), "a b");
        assert_eq!(urldecode("%zz"), "%zz", "bad escapes are left alone");
        assert_eq!(urldecode("%"), "%");
    }

    #[test]
    fn the_done_page_never_closes_over_stale_text() {
        let ok = done_page(true, true);
        assert!(ok.contains("connected"));
        assert!(ok.contains("close this tab"));
        let bad = done_page(false, true);
        assert!(bad.contains("did not finish"));
        assert!(
            !bad.contains("connected"),
            "a failure must not read as success"
        );
    }

    #[test]
    fn google_blocking_loopback_is_recognised_from_the_redirect() {
        // Observed from this exact client type: Google redirects a
        // "TVs and Limited Input devices" client to its error page with the
        // reason in the query. Treating that as success would open a browser
        // full of error and leave the loopback listener waiting for a callback
        // that will never come.
        assert!(location_means_blocked(
            "https://accounts.google.com/signin/oauth/error?authError=Cg9pbnZhbGlk"
        ));
        assert!(location_means_blocked(
            "https://accounts.google.com/o/oauth2/auth?authError=xyz"
        ));
        // A good desktop client goes to the chooser, or straight back to the
        // loopback if a session is already signed in.
        assert!(!location_means_blocked(
            "https://accounts.google.com/o/oauth2/v2/auth?flowName=GeneralOAuth"
        ));
        assert!(!location_means_blocked(
            "http://127.0.0.1:53682/?code=abc&state=s"
        ));
        // No redirect at all is not evidence of success.
        assert!(!location_means_blocked(""));
    }
}
