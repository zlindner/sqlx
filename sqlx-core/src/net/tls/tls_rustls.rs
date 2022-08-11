use futures_util::future;
use std::io;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::SystemTime;

use rustls::{
    client::{ServerCertVerified, ServerCertVerifier, WebPkiVerifier},
    ClientConfig, ClientConnection, Error as TlsError, OwnedTrustAnchor, RootCertStore, ServerName,
};

use crate::error::Error;
use crate::io::ReadBuf;
use crate::net::tls::util::StdSocket;
use crate::net::tls::{CertificateInput, TlsConfig};
use crate::net::{Socket, WithSocket};

pub struct RustlsSocket<S: Socket> {
    inner: StdSocket<S>,
    state: ClientConnection,
    close_notify_sent: bool,
}

impl<S: Socket> RustlsSocket<S> {
    fn poll_complete_io(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match self.state.complete_io(&mut self.inner) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    futures_util::ready!(self.inner.poll_ready(cx))?;
                }
                ready => return Poll::Ready(ready.map(|_| ())),
            }
        }
    }

    async fn complete_io(&mut self) -> io::Result<()> {
        future::poll_fn(|cx| self.poll_complete_io(cx)).await
    }
}

impl<S: Socket> Socket for RustlsSocket<S> {
    fn try_read(&mut self, buf: &mut dyn ReadBuf) -> io::Result<usize> {
        self.state.reader().read(buf.init_mut())
    }

    fn try_write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.state.writer().write(buf)
    }

    fn poll_read_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_complete_io(cx)
    }

    fn poll_write_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_complete_io(cx)
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.close_notify_sent {
            self.state.send_close_notify();
            self.close_notify_sent = true;
        }

        futures_util::ready!(self.poll_complete_io(cx))?;
        self.inner.socket.poll_shutdown(cx)
    }
}

pub async fn handshake<S>(socket: S, tls_config: TlsConfig<'_>) -> Result<RustlsSocket<S>, Error>
where
    S: Socket,
{
    let config = ClientConfig::builder().with_safe_defaults();

    let config = if tls_config.accept_invalid_certs {
        config
            .with_custom_certificate_verifier(Arc::new(DummyTlsVerifier))
            .with_no_client_auth()
    } else {
        let mut cert_store = RootCertStore::empty();
        cert_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));

        if let Some(ca) = tls_config.root_cert_path {
            let data = ca.data().await?;
            let mut cursor = Cursor::new(data);

            for cert in rustls_pemfile::certs(&mut cursor)
                .map_err(|_| Error::Tls(format!("Invalid certificate {}", ca).into()))?
            {
                cert_store
                    .add(&rustls::Certificate(cert))
                    .map_err(|err| Error::Tls(err.into()))?;
            }
        }

        if tls_config.accept_invalid_hostnames {
            let verifier = WebPkiVerifier::new(cert_store, None);

            config
                .with_custom_certificate_verifier(Arc::new(NoHostnameTlsVerifier { verifier }))
                .with_no_client_auth()
        } else {
            config
                .with_root_certificates(cert_store)
                .with_no_client_auth()
        }
    };

    let host = rustls::ServerName::try_from(tls_config.hostname).map_err(Error::tls)?;

    let mut socket = RustlsSocket {
        inner: StdSocket::new(socket),
        state: ClientConnection::new(Arc::new(config), host).map_err(Error::tls)?,
        close_notify_sent: false,
    };

    // Performs the TLS handshake or bails
    socket.complete_io().await?;

    Ok(socket)
}

struct DummyTlsVerifier;

impl ServerCertVerifier for DummyTlsVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
}

pub struct NoHostnameTlsVerifier {
    verifier: WebPkiVerifier,
}

impl ServerCertVerifier for NoHostnameTlsVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::Certificate,
        intermediates: &[rustls::Certificate],
        server_name: &ServerName,
        scts: &mut dyn Iterator<Item = &[u8]>,
        ocsp_response: &[u8],
        now: SystemTime,
    ) -> Result<ServerCertVerified, TlsError> {
        match self.verifier.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        ) {
            Err(TlsError::InvalidCertificateData(reason))
                if reason.contains("CertNotValidForName") =>
            {
                Ok(ServerCertVerified::assertion())
            }
            res => res,
        }
    }
}
