use anyhow::{Result, ensure};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, SecretKey, endpoint::presets};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const ALPN: &[u8] = b"anywhere/0.4";
pub const MAX_FRAME: usize = 20 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
pub fn new_secret() -> [u8; 32] {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).expect("OS randomness");
    bytes
}
pub fn validate_relay(url: &str) -> Result<iroh::RelayUrl> {
    let parsed: iroh::RelayUrl = url.parse()?;
    ensure!(
        url.starts_with("https://") || url.starts_with("http://127.0.0.1:"),
        "Relay requires TLS"
    );
    ensure!(
        !url.contains('@') && !url.contains('?') && !url.contains('#'),
        "Relay URL must not contain credentials"
    );
    Ok(parsed)
}
pub async fn endpoint(secret: SecretKey, relays: &[String]) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()]);
    if relays.is_empty() {
        builder = builder.relay_mode(RelayMode::Custom(RelayMap::empty()));
    } else {
        let urls = relays
            .iter()
            .map(|s| validate_relay(s))
            .collect::<Result<Vec<_>>>()?;
        builder = builder.relay_mode(RelayMode::Custom(RelayMap::from_iter(urls)));
    }
    Ok(builder.bind().await?)
}
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    read_frame_limit(reader, MAX_FRAME).await
}
pub async fn read_frame_limit<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Vec<u8>> {
    let size = reader.read_u32().await? as usize;
    ensure!(size <= limit, "Frame too large");
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_FRAME, "Frame too large");
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(bytes).await?;
    Ok(())
}
pub async fn write_json<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &impl Serialize,
) -> Result<()> {
    write_frame(writer, &serde_json::to_vec(value)?).await
}
pub async fn call(connection: &iroh::endpoint::Connection, request: &Request) -> Result<Value> {
    let (mut send, mut recv) = connection.open_bi().await?;
    write_json(&mut send, request).await?;
    send.finish()?;
    Ok(serde_json::from_slice(&read_frame(&mut recv).await?)?)
}
pub fn address(id: &str, relays: &[String]) -> Result<EndpointAddr> {
    let mut addr = EndpointAddr::new(id.parse()?);
    for relay in relays {
        addr = addr.with_relay_url(validate_relay(relay)?);
    }
    Ok(addr)
}

pub mod admission;
mod diagnostics;
pub use diagnostics::connection_status;
mod ffi;
