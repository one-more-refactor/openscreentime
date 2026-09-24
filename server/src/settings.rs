//! The server's public identity, derived from one variable.
//!
//! An operator sets `OST_PUBLIC_URL` (the https address their reverse proxy
//! serves) and everything browser-facing follows from it: the WebAuthn
//! relying-party id (its host) and origin, the CORS origin, and whether
//! cookies must be `Secure` (https → yes). `RP_ID`, `RP_ORIGIN` and
//! `OST_INSECURE_COOKIES` still work as explicit overrides; empty values count
//! as unset, like every other optional setting (`state::configured`).

use anyhow::Context;
use url::Url;

/// Plain `cargo run` against the Vite dev server, when nothing is configured.
const DEV_ORIGIN: &str = "http://localhost:5173";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSettings {
    /// Base URL for OIDC redirects and post-login redirects (no trailing slash).
    pub public_url: String,
    /// The exact origin browsers see: `scheme://host[:port]`.
    pub rp_origin: String,
    /// The WebAuthn relying-party id: the bare host.
    pub rp_id: String,
    /// Session cookies carry `Secure`.
    pub cookie_secure: bool,
}

impl PublicSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        use crate::state::configured;
        Self::derive(
            configured("OST_PUBLIC_URL"),
            configured("RP_ORIGIN"),
            configured("RP_ID"),
            configured("OST_INSECURE_COOKIES"),
        )
    }

    pub fn derive(
        public_url: Option<String>,
        rp_origin: Option<String>,
        rp_id: Option<String>,
        insecure_cookies: Option<String>,
    ) -> anyhow::Result<Self> {
        // An explicit RP_ORIGIN wins; otherwise the public URL's origin.
        let origin_src = rp_origin
            .or_else(|| public_url.clone())
            .unwrap_or_else(|| DEV_ORIGIN.to_string());
        let origin_url = Url::parse(&origin_src)
            .with_context(|| format!("OST_PUBLIC_URL / RP_ORIGIN is not a URL: {origin_src}"))?;
        if !matches!(origin_url.scheme(), "http" | "https") {
            anyhow::bail!("OST_PUBLIC_URL / RP_ORIGIN must be http(s): {origin_src}");
        }
        let host = origin_url
            .host_str()
            .with_context(|| format!("OST_PUBLIC_URL / RP_ORIGIN has no host: {origin_src}"))?
            .to_string();
        let rp_origin = origin_url.origin().ascii_serialization();
        let rp_id = rp_id.unwrap_or(host);
        let public_url = public_url
            .unwrap_or_else(|| rp_origin.clone())
            .trim_end_matches('/')
            .to_string();
        // Secure cookies are what https needs and what plain http can't store,
        // so the scheme decides — unless the operator said otherwise.
        let cookie_secure = match insecure_cookies.as_deref().map(str::to_ascii_lowercase) {
            Some(v) if matches!(v.as_str(), "1" | "true" | "yes") => false,
            Some(v) if matches!(v.as_str(), "0" | "false" | "no") => true,
            _ => origin_url.scheme() == "https",
        };
        Ok(PublicSettings {
            public_url,
            rp_origin,
            rp_id,
            cookie_secure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PublicSettings;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn one_public_url_is_enough() {
        let p = PublicSettings::derive(s("https://ost.example.com"), None, None, None).unwrap();
        assert_eq!(p.public_url, "https://ost.example.com");
        assert_eq!(p.rp_origin, "https://ost.example.com");
        assert_eq!(p.rp_id, "ost.example.com");
        assert!(p.cookie_secure);
    }

    #[test]
    fn trailing_slash_and_port_are_handled() {
        let p =
            PublicSettings::derive(s("https://ost.example.com:8443/"), None, None, None).unwrap();
        assert_eq!(p.public_url, "https://ost.example.com:8443");
        assert_eq!(p.rp_origin, "https://ost.example.com:8443");
        assert_eq!(p.rp_id, "ost.example.com");
    }

    #[test]
    fn explicit_overrides_still_win() {
        let p = PublicSettings::derive(
            s("https://ost.example.com"),
            s("https://login.example.com"),
            s("example.com"),
            s("1"),
        )
        .unwrap();
        assert_eq!(p.public_url, "https://ost.example.com");
        assert_eq!(p.rp_origin, "https://login.example.com");
        assert_eq!(p.rp_id, "example.com");
        assert!(!p.cookie_secure);
    }

    /// An existing .env that sets only the old pair keeps working unchanged.
    #[test]
    fn legacy_rp_origin_alone_still_works() {
        let p = PublicSettings::derive(
            None,
            s("https://ost.example.com"),
            s("ost.example.com"),
            None,
        )
        .unwrap();
        assert_eq!(p.public_url, "https://ost.example.com");
        assert_eq!(p.rp_id, "ost.example.com");
        assert!(p.cookie_secure);
    }

    #[test]
    fn plain_http_dev_gets_cookies_it_can_store() {
        let p = PublicSettings::derive(None, None, None, None).unwrap();
        assert_eq!(p.rp_origin, "http://localhost:5173");
        assert_eq!(p.rp_id, "localhost");
        assert!(!p.cookie_secure);
        // …unless the operator insists.
        let p = PublicSettings::derive(s("http://localhost:8080"), None, None, s("0")).unwrap();
        assert!(p.cookie_secure);
    }

    #[test]
    fn garbage_is_refused_not_guessed() {
        assert!(PublicSettings::derive(s("ost.example.com"), None, None, None).is_err());
        assert!(PublicSettings::derive(s("ftp://ost.example.com"), None, None, None).is_err());
    }
}
