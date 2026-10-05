use crate::model::{Media, confident, score};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{net::IpAddr, process::Stdio, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, sync::Semaphore};
use url::Url;

const OUTPUT_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct Resolver {
    gate: Arc<Semaphore>,
    requests: Arc<Semaphore>,
}
pub enum Resolution {
    Batch(Vec<Media>),
    Choices(Vec<Media>),
}
pub struct Stream {
    pub url: String,
    pub headers: String,
    pub duration: Option<f64>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self {
            gate: Arc::new(Semaphore::new(1)),
            requests: Arc::new(Semaphore::new(8)),
        }
    }
}

fn supported_url(query: &str) -> Result<Url> {
    let url = Url::parse(query).context("Invalid media URL.")?;
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let allowed = ["youtube.com", "youtu.be", "soundcloud.com", "bandcamp.com"];
    if !allowed
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        bail!(
            "This provider has no verified playback adapter yet. Use YouTube, SoundCloud or Bandcamp."
        );
    }
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        bail!("Use a public provider URL without credentials or a custom port.");
    }
    Ok(url)
}

fn canonical(entry: &Value, fallback: &str) -> Option<String> {
    let raw = entry
        .get("webpage_url")
        .or_else(|| entry.get("url"))
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    if supported_url(raw).is_ok() {
        return Some(raw.into());
    }
    let extractor = entry
        .get("ie_key")
        .or_else(|| entry.get("extractor_key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if extractor.to_ascii_lowercase().contains("youtube") {
        let id = entry.get("id")?.as_str()?;
        if id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Some(format!("https://www.youtube.com/watch?v={id}"));
        }
    }
    None
}

fn media(entry: &Value, fallback: &str) -> Option<Media> {
    if entry
        .get("availability")
        .and_then(Value::as_str)
        .is_some_and(|s| matches!(s, "private" | "premium_only" | "subscriber_only"))
    {
        return None;
    }
    let title = entry.get("title")?.as_str()?.to_owned();
    if title == "[Deleted video]" || title == "[Private video]" {
        return None;
    }
    let live = entry
        .get("is_live")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || entry.get("live_status").and_then(Value::as_str) == Some("is_live");
    Some(Media {
        title,
        artist: entry
            .get("artist")
            .or_else(|| entry.get("uploader"))
            .or_else(|| entry.get("channel"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown artist")
            .into(),
        url: canonical(entry, fallback)?,
        duration: if live {
            None
        } else {
            entry
                .get("duration")
                .and_then(Value::as_f64)
                .filter(|d| d.is_finite() && *d > 0.0)
        },
        verified: entry
            .get("channel_is_verified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl Resolver {
    async fn extract(&self, query: &str, flat: bool) -> Result<Value> {
        let _permit = tokio::time::timeout(Duration::from_secs(45), self.gate.acquire())
            .await
            .context("Extractor is busy. Try again shortly.")??;
        let mut command = Command::new("yt-dlp");
        command.args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-warnings",
            "--no-progress",
            "--skip-download",
            "--socket-timeout",
            "15",
            "--retries",
            "1",
            "--js-runtimes",
            "node",
            "--dump-single-json",
        ]);
        if flat {
            command.args(["--flat-playlist", "--playlist-end", "200"]);
        } else {
            command.args(["--no-playlist", "--format", "bestaudio/best"]);
        }
        let mut child = command
            .arg("--")
            .arg(query)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("yt-dlp is unavailable.")?;
        let stdout = child
            .stdout
            .take()
            .context("Extractor output pipe missing.")?;
        let mut stderr = child
            .stderr
            .take()
            .context("Extractor error pipe missing.")?;
        let work = async {
            let read = async {
                let mut bytes = Vec::new();
                stdout
                    .take(OUTPUT_LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() as u64 > OUTPUT_LIMIT {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Provider metadata exceeded the safety limit.",
                    ));
                }
                Ok::<_, std::io::Error>(bytes)
            };
            // Drain stderr without retaining signed URLs, request headers or cookies.
            let drain = async { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await };
            let (bytes, _, status) = tokio::try_join!(read, drain, child.wait())?;
            if bytes.len() as u64 > OUTPUT_LIMIT {
                bail!("Provider metadata exceeded the safety limit.");
            }
            if !status.success() {
                bail!(
                    "Provider extraction failed. The media may be restricted, unavailable or require provider authentication."
                );
            }
            serde_json::from_slice(&bytes).context("Provider returned invalid metadata.")
        };
        tokio::time::timeout(Duration::from_secs(45), work)
            .await
            .context("Provider extraction timed out.")?
    }

    pub async fn resolve(&self, query: &str) -> Result<Resolution> {
        let _request = self
            .requests
            .try_acquire()
            .context("Too many pending searches. Try again shortly.")?;
        let query = query.trim();
        if query.is_empty() || query.len() > 500 {
            bail!("Use a query between 1 and 500 characters.");
        }
        let exact = query.starts_with("http://") || query.starts_with("https://");
        if exact {
            supported_url(query)?;
        } else if query.contains("://") {
            bail!("Invalid provider URL.");
        }
        let search = if exact {
            query.to_owned()
        } else {
            format!("ytsearch10:{query}")
        };
        let json = self.extract(&search, true).await?;
        let mut tracks = if let Some(entries) = json.get("entries").and_then(Value::as_array) {
            entries
                .iter()
                .filter_map(|v| media(v, ""))
                .collect::<Vec<_>>()
        } else {
            media(&json, query).into_iter().collect()
        };
        if tracks.is_empty() {
            bail!("No playable public tracks were found.");
        }
        if exact {
            return Ok(Resolution::Batch(tracks));
        }
        tracks.sort_by(|a, b| score(query, b).total_cmp(&score(query, a)));
        let scores = tracks.iter().map(|m| score(query, m)).collect::<Vec<_>>();
        if confident(&scores) {
            Ok(Resolution::Batch(vec![tracks.remove(0)]))
        } else {
            tracks.truncate(5);
            Ok(Resolution::Choices(tracks))
        }
    }

    pub async fn stream(&self, media: &Media) -> Result<Stream> {
        supported_url(&media.url)?;
        let json = self.extract(&media.url, false).await?;
        let raw = json
            .get("url")
            .and_then(Value::as_str)
            .context("Provider supplied no playable stream.")?;
        validate_stream(raw).await?;
        let mut headers = String::new();
        if let Some(map) = json.get("http_headers").and_then(Value::as_object) {
            for key in ["User-Agent", "Referer", "Origin"] {
                if let Some(value) = map.get(key).and_then(Value::as_str) {
                    if value.len() > 2048 || value.contains(['\r', '\n']) {
                        bail!("Invalid provider headers.");
                    }
                    headers.push_str(&format!("{key}: {value}\r\n"));
                }
            }
        }
        Ok(Stream {
            url: raw.into(),
            headers,
            duration: media_from_stream(&json),
        })
    }
}

fn media_from_stream(json: &Value) -> Option<f64> {
    if json.get("is_live").and_then(Value::as_bool) == Some(true)
        || json.get("live_status").and_then(Value::as_str) == Some("is_live")
    {
        None
    } else {
        json.get("duration")
            .and_then(Value::as_f64)
            .filter(|d| d.is_finite() && *d > 0.0)
    }
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1])))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                public_ip(IpAddr::V4(v4))
            } else {
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_multicast())
            }
        }
    }
}

async fn validate_stream(raw: &str) -> Result<()> {
    let url = Url::parse(raw).context("Invalid stream URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.port_or_known_default(), Some(80 | 443))
    {
        bail!("Unsupported stream protocol.");
    }
    let host = url.host_str().context("Missing stream host.")?;
    let addresses = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, url.port_or_known_default().unwrap())),
    )
    .await
    .context("Stream DNS timed out.")??
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        bail!("Provider supplied a non-public stream address.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_hosts_are_exact_or_subdomains() {
        for url in [
            "https://youtube.com.evil.test/x",
            "file:///tmp/x",
            "https://user:pass@youtube.com/x",
            "https://youtube.com:123/x",
            "https://open.spotify.com/track/x",
        ] {
            assert!(supported_url(url).is_err());
        }
        assert!(supported_url("https://music.youtube.com/watch?v=x").is_ok());
    }
    #[test]
    fn private_stream_ips_are_rejected() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.2",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()));
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
    }
    #[test]
    fn live_and_missing_duration_stay_unknown() {
        let v = serde_json::json!({"id":"abc","ie_key":"Youtube","title":"live","duration":90,"is_live":true});
        assert!(media(&v, "").unwrap().duration.is_none());
    }
}
