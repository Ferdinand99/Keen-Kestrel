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

    /// A second port that serves only ingest (and /healthz), so the admin
    /// interface can stay on a LAN or VPN. Ingest stays on the main port too.
    #[arg(long, env = "KK_INGEST_PORT")]
    pub ingest_port: Option<u16>,

    /// Items per minute one IP address may send to ingest (0: no limit): an
    /// envelope counts for each of its items, and an IPv6 address stands for
    /// its /64. Kept in memory only.
    #[arg(long, env = "KK_INGEST_IP_RATE_LIMIT", default_value_t = 6000)]
    pub ingest_ip_rate_limit: u32,

    /// Size in MB that store.sqlite (event details, attachments, raw
    /// analytics) may reach before ingest stops keeping new details and
    /// answers web events and app signals with 429; errors are still counted
    /// (0: no limit). Ingest does the same when the data volume has less
    /// than 1 GiB free.
    #[arg(long, env = "KK_MAX_STORE_MB", default_value_t = 0)]
    pub max_store_mb: u64,

    /// Write every ingest request (headers and body, as sent) to this
    /// directory, to record SDK fixtures. For test instances only: the files
    /// hold everything the SDKs send.
    #[arg(long, env = "KESTREL_CAPTURE_DIR")]
    pub capture_dir: Option<PathBuf>,
}

impl Config {
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    /// Where the ingest-only listener binds, when there is one.
    pub fn ingest_addr(&self) -> Option<SocketAddr> {
        self.ingest_port
            .map(|port| SocketAddr::new(self.host, port))
    }

    /// Ingest's settings from the flags.
    pub fn ingest_settings(&self) -> crate::ingest::Settings {
        crate::ingest::Settings {
            capture_dir: self.capture_dir.clone(),
            ip_rate_limit_per_minute: self.ingest_ip_rate_limit,
            writer: crate::writer::WriterSettings {
                max_store_bytes: (self.max_store_mb > 0)
                    .then(|| self.max_store_mb.saturating_mul(1024 * 1024)),
                ..crate::writer::WriterSettings::default()
            },
            ..crate::ingest::Settings::default()
        }
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
        assert_eq!(c.ingest_addr(), None);
        assert_eq!(c.ingest_settings().ip_rate_limit_per_minute, 6000);
        assert_eq!(c.ingest_settings().writer.max_store_bytes, None);
        let capped = Config::try_parse_from(["kestrel", "--max-store-mb", "512"]).unwrap();
        assert_eq!(
            capped.ingest_settings().writer.max_store_bytes,
            Some(512 * 1024 * 1024)
        );
        let c = Config::try_parse_from([
            "kestrel",
            "--port",
            "8080",
            "--public-url",
            "https://errors.example.com/",
            "--ingest-port",
            "3003",
            "--capture-dir",
            "/tmp/cap",
        ])
        .unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.ingest_addr().unwrap().to_string(), "0.0.0.0:3003");
        assert_eq!(
            c.ingest_settings().capture_dir,
            Some(PathBuf::from("/tmp/cap"))
        );
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
