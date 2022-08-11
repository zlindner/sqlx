#![allow(dead_code)]

use std::io;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::error::Error;
use crate::net::socket::WithSocket;
use crate::net::Socket;
use std::mem::replace;

#[cfg(feature = "_tls-rustls")]
mod tls_rustls;

#[cfg(feature = "_tls-native-tls")]
mod tls_native_tls;

mod util;

/// X.509 Certificate input, either a file path or a PEM encoded inline certificate(s).
#[derive(Clone, Debug)]
pub enum CertificateInput {
    /// PEM encoded certificate(s)
    Inline(Vec<u8>),
    /// Path to a file containing PEM encoded certificate(s)
    File(PathBuf),
}

impl From<String> for CertificateInput {
    fn from(value: String) -> Self {
        let trimmed = value.trim();
        // Some heuristics according to https://tools.ietf.org/html/rfc7468
        if trimmed.starts_with("-----BEGIN CERTIFICATE-----")
            && trimmed.contains("-----END CERTIFICATE-----")
        {
            CertificateInput::Inline(value.as_bytes().to_vec())
        } else {
            CertificateInput::File(PathBuf::from(value))
        }
    }
}

impl CertificateInput {
    async fn data(&self) -> Result<Vec<u8>, std::io::Error> {
        use crate::fs;
        match self {
            CertificateInput::Inline(v) => Ok(v.clone()),
            CertificateInput::File(path) => fs::read(path).await,
        }
    }
}

impl std::fmt::Display for CertificateInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertificateInput::Inline(v) => write!(f, "{}", String::from_utf8_lossy(v.as_slice())),
            CertificateInput::File(path) => write!(f, "file: {}", path.display()),
        }
    }
}

pub struct TlsConfig<'a> {
    pub accept_invalid_certs: bool,
    pub accept_invalid_hostnames: bool,
    pub hostname: &'a str,
    pub root_cert_path: Option<&'a CertificateInput>,
}

pub async fn handshake<S, Ws>(
    socket: S,
    config: TlsConfig<'_>,
    with_socket: Ws,
) -> crate::Result<Ws::Output>
where
    S: Socket,
    Ws: WithSocket,
{
    #[cfg(feature = "_tls-native-tls")]
    return Ok(with_socket.with_socket(tls_native_tls::handshake(socket, config).await?));

    #[cfg(feature = "_tls-rustls")]
    return Ok(with_socket.with_socket(tls_rustls::handshake(socket, config).await?));

    panic!("one of the `runtime-*-native-tls` or `runtime-*-rustls` features must be enabled")
}

pub fn available() -> bool {
    cfg!(any(feature = "_tls-native-tls", feature = "_tls-rustls"))
}
