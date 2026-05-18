//! Codex-compatible request headers for the `chatgpt.com/backend-api/codex` endpoint.
//!
//! When `codex_auth` is enabled, LunaRoute masquerades as the Codex CLI so the
//! upstream (Cloudflare-fronted) accepts the request. We mirror two headers the
//! real Codex client sets on every request:
//!
//! - `User-Agent`: `codex_cli_rs/{version} ({OS}; {arch}) {terminal_token}`
//! - `originator`: `codex_cli_rs`
//!
//! Format reference:
//! https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/default_client.rs
//!
//! Note: matching these headers alone does NOT defeat Cloudflare's JA3
//! fingerprinting (see openai/codex#17860). Header matching is defense-in-depth;
//! the real workaround for Linux is pointing at `api.openai.com` instead.

/// Originator header value sent by the real Codex CLI.
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";

/// Default Codex CLI version used when `LUNAROUTE_CODEX_UA_VERSION` is unset.
/// Bump periodically to stay plausible; Cloudflare probably doesn't parse it.
const DEFAULT_CODEX_VERSION: &str = "0.118.0";

/// Build a Codex-compatible `User-Agent` header value.
///
/// The version can be overridden via the `LUNAROUTE_CODEX_UA_VERSION` env var.
/// OS and architecture are read from `std::env::consts` at runtime.
pub fn codex_user_agent() -> String {
    let version = std::env::var("LUNAROUTE_CODEX_UA_VERSION")
        .unwrap_or_else(|_| DEFAULT_CODEX_VERSION.to_string());
    let os = os_name();
    let arch = std::env::consts::ARCH;
    // `xterm-256color` is the most common terminal token Codex emits when
    // TERM_PROGRAM is unset. We use a static value rather than sniffing the
    // environment — this is a proxy, not a terminal.
    format!("codex_cli_rs/{version} ({os}; {arch}) xterm-256color")
}

/// Map Rust's OS constant to the capitalization `os_info::os_type()` produces,
/// which is what Codex embeds in its UA.
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Mac OS",
        "windows" => "Windows",
        "freebsd" => "FreeBSD",
        "netbsd" => "NetBSD",
        "openbsd" => "OpenBSD",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_has_codex_prefix() {
        let ua = codex_user_agent();
        assert!(ua.starts_with("codex_cli_rs/"), "got: {ua}");
    }

    #[test]
    fn user_agent_contains_arch() {
        let ua = codex_user_agent();
        assert!(ua.contains(std::env::consts::ARCH), "got: {ua}");
    }

    #[test]
    fn user_agent_uses_env_override() {
        // SAFETY: tests in this module are not parallel with anything else
        // that reads this env var.
        unsafe { std::env::set_var("LUNAROUTE_CODEX_UA_VERSION", "9.9.9") };
        let ua = codex_user_agent();
        unsafe { std::env::remove_var("LUNAROUTE_CODEX_UA_VERSION") };
        assert!(ua.contains("codex_cli_rs/9.9.9"), "got: {ua}");
    }

    #[test]
    fn os_name_maps_known_values() {
        // At least one of these maps matches the current host.
        let mapped = os_name();
        assert!(!mapped.is_empty());
        // Ensure we always return something HTTP-header safe.
        assert!(mapped.chars().all(|c| c.is_ascii_graphic() || c == ' '));
    }
}
