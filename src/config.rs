//! Process configuration: command-line flags, each with a `KK_` environment
//! variable. Everything else lives in the database.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;

/// Keen Kestrel: self-hosted error tracking and privacy-friendly analytics.
#[derive(Debug, Clone, Parser)]
#[command(name = "kestrel", version, about, long_about = None)]
pub struct Config {
    /// Directory for the database (kestrel.sqlite), the session key and
    /// pre-migration backups.
    #[arg(long, env = "KK_DATA_DIR", default_value = "./data")]
    pub data_dir: PathBuf,

    /// Address to listen on.
    #[arg(long, env = "KK_HOST", default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
    pub host: IpAddr,

    /// Port to listen on.
    #[arg(long, env = "KK_PORT", default_value_t = 3002)]
    pub port: u16,

    /// The address people and SDKs use to reach this instance, like
    /// https://kestrel.example.com. Used in DSNs, API examples and the
    /// same-origin checks; without it, the address of each request is used.
    #[arg(long, env = "KK_PUBLIC_URL", value_parser = parse_public_url)]
    pub public_url: Option<String>,
}

impl Config {
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

/// An http(s) URL without query or fragment, returned without a trailing slash.
fn parse_public_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let url = url::Url::parse(raw).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("use an http:// or https:// address, like https://kestrel.example.com".into());
    }
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
        return Err("leave out user names, queries and fragments".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_defaults() {
        let c = Config::try_parse_from(["kestrel"]).unwrap();
        assert_eq!(c.port, 3002);
        assert_eq!(c.bind_addr().to_string(), "0.0.0.0:3002");
        let c = Config::try_parse_from([
            "kestrel",
            "--port",
            "8080",
            "--public-url",
            "https://errors.example.com/",
        ])
        .unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.public_url.as_deref(), Some("https://errors.example.com"));
    }

    #[test]
    fn public_urls_are_checked() {
        assert!(parse_public_url("https://example.com/kestrel/").is_ok());
        for bad in ["example.com", "ftp://x", "https://x?y=1", "https://u@x"] {
            assert!(parse_public_url(bad).is_err(), "{bad}");
        }
    }
}
