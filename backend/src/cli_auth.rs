use anyhow::{anyhow, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use tracing::info;

/// Check if the remote Phos server requires SSO authentication.
/// If so, perform a browser-based OIDC login and return the Phos JWT token.
/// Returns `None` if no auth is required (single-user mode).
pub fn authenticate_if_needed(server_url: &str) -> anyhow::Result<Option<String>> {
    let config_url = format!("{}/api/auth/config", server_url.trim_end_matches('/'));
    let client = ureq::Agent::new_with_defaults();

    // Fetch OIDC config — if the endpoint doesn't exist, SSO is not enabled.
    let mut config_resp = match client.get(&config_url).call() {
        Ok(resp) => resp,
        Err(_) => {
            info!("Auth config not available, proceeding without auth");
            return Ok(None);
        }
    };

    let config: serde_json::Value = config_resp
        .body_mut()
        .read_json()
        .context("Failed to parse auth config")?;

    let issuer = config["issuer"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing issuer in auth config"))?;
    let client_id = config["client_id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing client_id in auth config"))?;
    let scopes: Vec<&str> = config["scopes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_else(|| vec!["openid", "profile", "email"]);

    // OIDC discovery
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let discovery: serde_json::Value = client
        .get(&discovery_url)
        .call()
        .context("OIDC discovery failed")?
        .body_mut()
        .read_json()
        .context("Failed to parse OIDC discovery document")?;

    let authorization_endpoint = discovery["authorization_endpoint"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing authorization_endpoint in OIDC discovery"))?;
    let token_endpoint = discovery["token_endpoint"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing token_endpoint in OIDC discovery"))?;

    // Generate PKCE pair
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);

    // CSRF state
    let state = generate_random_string();

    // Start local listener for the OIDC callback
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://localhost:{}/callback", port);

    // Build authorization URL
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        authorization_endpoint,
        urlencoding::encode(client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scopes.join(" ")),
        urlencoding::encode(&code_challenge),
        urlencoding::encode(&state),
    );

    println!("Opening browser for authentication...");
    println!(
        "If the browser doesn't open, visit:\n  {}\n",
        auth_url
    );
    open_browser(&auth_url);

    // Wait for the OIDC provider to redirect back to our listener
    let (code, returned_state) = wait_for_callback(&listener)?;

    if returned_state != state {
        anyhow::bail!("CSRF state mismatch — possible attack or stale login attempt");
    }

    // Exchange authorization code for tokens at the OIDC provider
    let mut token_resp = client
        .post(token_endpoint)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id),
            ("code_verifier", code_verifier.as_str()),
        ])
        .context("Token exchange with OIDC provider failed")?;

    let token_json: serde_json::Value = token_resp
        .body_mut()
        .read_json()
        .context("Failed to parse OIDC token response")?;

    let id_token = token_json["id_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No id_token in OIDC token response"))?;

    // Exchange ID token for a Phos session JWT
    let phos_token_url = format!("{}/api/auth/token", server_url.trim_end_matches('/'));
    let mut phos_resp = client
        .post(&phos_token_url)
        .send_json(&serde_json::json!({ "id_token": id_token }))
        .context("Phos token exchange failed")?;

    let phos_json: serde_json::Value = phos_resp
        .body_mut()
        .read_json()
        .context("Failed to parse Phos token response")?;

    let token = phos_json["token"]
        .as_str()
        .ok_or_else(|| anyhow!("No token in Phos token response"))?
        .to_string();

    println!("Authentication successful!");
    Ok(Some(token))
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/// Generate a random PKCE code verifier (43–128 chars, base64url-encoded).
fn generate_code_verifier() -> String {
    let bytes = random_bytes(32);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive the PKCE code challenge from a code verifier (S256 method).
fn generate_code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a random base64url string for CSRF state.
fn generate_random_string() -> String {
    let bytes = random_bytes(32);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate `n` cryptographically random bytes using UUID v4 (avoids extra dep on `rand`).
fn random_bytes(n: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(n);
    while result.len() < n {
        // UUID v4 uses the platform's CSRNG for 122 bits of randomness per call.
        let id = uuid::Uuid::new_v4();
        result.extend_from_slice(id.as_bytes());
    }
    result.truncate(n);
    result
}

// ---------------------------------------------------------------------------
// Browser + callback
// ---------------------------------------------------------------------------

fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Block until the OIDC provider redirects the browser to our local listener.
/// Returns `(authorization_code, state)`.
fn wait_for_callback(listener: &TcpListener) -> anyhow::Result<(String, String)> {
    let (mut stream, _) = listener.accept()?;

    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse: GET /callback?code=xxx&state=yyy HTTP/1.1
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow!("Empty HTTP request from browser callback"))?;
    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("Malformed HTTP request from browser callback"))?;

    let query = path
        .split('?')
        .nth(1)
        .ok_or_else(|| anyhow!("No query parameters in callback URL"))?;

    let params: std::collections::HashMap<&str, &str> = query
        .split('&')
        .filter_map(|p| {
            let mut parts = p.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    // Check for OIDC error response
    if let Some(error) = params.get("error") {
        let desc = params.get("error_description").unwrap_or(&"");
        let decoded_desc = urlencoding::decode(desc).unwrap_or_default();

        send_html(
            &mut stream,
            &format!(
                "<h1>Authentication Failed</h1><p>{}: {}</p><p>You can close this tab.</p>",
                error, decoded_desc
            ),
        );

        anyhow::bail!("OIDC error: {} — {}", error, decoded_desc);
    }

    let code = params
        .get("code")
        .ok_or_else(|| anyhow!("No authorization code in callback"))?;
    let state = params
        .get("state")
        .ok_or_else(|| anyhow!("No state in callback"))?;

    let code = urlencoding::decode(code)?.into_owned();
    let state = urlencoding::decode(state)?.into_owned();

    send_html(
        &mut stream,
        "<h1>Authentication Successful</h1><p>You can close this tab and return to the terminal.</p>",
    );

    Ok((code, state))
}

fn send_html(stream: &mut impl Write, body: &str) {
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Phos</title></head><body>{}</body></html>",
        body
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
