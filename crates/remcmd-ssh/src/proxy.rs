use std::{
    io,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use remcmd_core::ConnectionProfile;
use secrecy::ExposeSecret;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

use crate::{
    ConnectionStage, RuntimeProxy, SshError, SshErrorKind,
    plan::{expand_proxy_command, proxy_command_approval_digest},
};

const MAX_PROXY_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_PROXY_COMMAND_STDERR_BYTES: usize = 16 * 1024;

pub(crate) trait AsyncStream: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncStream for T {}
pub(crate) type BoxedStream = Box<dyn AsyncStream + Send + Unpin>;

pub(crate) async fn open_initial_stream(
    proxy: Option<&RuntimeProxy>,
    endpoint: &ConnectionProfile,
) -> Result<BoxedStream, SshError> {
    let started_at = tokio::time::Instant::now();
    let proxy_kind = match proxy {
        None => "direct",
        Some(RuntimeProxy::HttpConnect { .. }) => "http_connect",
        Some(RuntimeProxy::Socks5 { .. }) => "socks5",
        Some(RuntimeProxy::ProxyCommand { .. }) => "proxy_command",
    };
    let result: Result<BoxedStream, SshError> = match proxy {
        None => super::transport::connect_tcp(&endpoint.host, endpoint.port)
            .await
            .map(|stream| Box::new(stream) as BoxedStream),
        Some(RuntimeProxy::HttpConnect {
            host,
            port,
            username,
            password,
        }) => open_http_connect(
            host,
            *port,
            username.as_deref(),
            password.as_ref().map(ExposeSecret::expose_secret),
            endpoint,
        )
        .await
        .map(|stream| Box::new(stream) as BoxedStream)
        .map_err(|error| error.at_stage(ConnectionStage::Proxy)),
        Some(RuntimeProxy::Socks5 {
            host,
            port,
            username,
            password,
        }) => open_socks5(
            host,
            *port,
            username.as_deref(),
            password.as_ref().map(ExposeSecret::expose_secret),
            endpoint,
        )
        .await
        .map(|stream| Box::new(stream) as BoxedStream)
        .map_err(|error| error.at_stage(ConnectionStage::Proxy)),
        Some(RuntimeProxy::ProxyCommand {
            command,
            approved_digest,
        }) => {
            let expected = proxy_command_approval_digest(command.expose_secret(), endpoint);
            if approved_digest.as_deref() != Some(expected.as_str()) {
                Err(SshError::new(
                    SshErrorKind::ProxyCommandApproval,
                    "ProxyCommand requires approval for the current target parameters",
                )
                .at_stage(ConnectionStage::Proxy))
            } else {
                match expand_proxy_command(command.expose_secret(), endpoint)
                    .map_err(|error| error.at_stage(ConnectionStage::Proxy))
                {
                    Ok(expanded) => open_proxy_command(&expanded)
                        .await
                        .map(|stream| Box::new(stream) as BoxedStream)
                        .map_err(|error| error.at_stage(ConnectionStage::Proxy)),
                    Err(error) => Err(error),
                }
            }
        }
    };
    match &result {
        Ok(_) => tracing::info!(
            stage = "proxy",
            proxy = proxy_kind,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            result = "success",
            "Initial transport stream established"
        ),
        Err(_) => tracing::warn!(
            stage = "proxy",
            proxy = proxy_kind,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            result = "failed",
            "Initial transport stream failed"
        ),
    }
    result
}

async fn open_http_connect(
    proxy_host: &str,
    proxy_port: u16,
    username: Option<&str>,
    password: Option<&str>,
    endpoint: &ConnectionProfile,
) -> Result<TcpStream, SshError> {
    let mut stream = super::transport::connect_tcp(proxy_host, proxy_port).await?;
    let authority = authority(&endpoint.host, endpoint.port);
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    match (username, password) {
        (Some(username), Some(password)) => {
            let credential = STANDARD.encode(format!("{username}:{password}"));
            request.push_str("Proxy-Authorization: Basic ");
            request.push_str(&credential);
            request.push_str("\r\n");
        }
        (None, None) => {}
        _ => {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                "HTTP CONNECT proxy authentication requires both username and password",
            ));
        }
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(io_error)?;

    let response = read_headers(&mut stream).await?;
    let status_line = response.lines().next().unwrap_or_default();
    let status = status_line
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| {
            SshError::new(
                SshErrorKind::Proxy,
                "HTTP CONNECT proxy returned an invalid response",
            )
        })?;
    match status {
        200..=299 => Ok(stream),
        407 => Err(SshError::new(
            SshErrorKind::ProxyAuthentication,
            "HTTP CONNECT proxy rejected authentication",
        )),
        _ => Err(SshError::new(
            SshErrorKind::Proxy,
            format!("HTTP CONNECT proxy returned status {status}"),
        )),
    }
}

async fn read_headers(stream: &mut TcpStream) -> Result<String, SshError> {
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while response.len() < MAX_PROXY_RESPONSE_BYTES {
        stream.read_exact(&mut byte).await.map_err(io_error)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            return String::from_utf8(response).map_err(|_| {
                SshError::new(
                    SshErrorKind::Proxy,
                    "HTTP CONNECT proxy response was not valid UTF-8",
                )
            });
        }
    }
    Err(SshError::new(
        SshErrorKind::Proxy,
        "HTTP CONNECT proxy response exceeded the size limit",
    ))
}

async fn open_socks5(
    proxy_host: &str,
    proxy_port: u16,
    username: Option<&str>,
    password: Option<&str>,
    endpoint: &ConnectionProfile,
) -> Result<TcpStream, SshError> {
    let mut stream = super::transport::connect_tcp(proxy_host, proxy_port).await?;
    let has_credentials = match (username, password) {
        (Some(_), Some(_)) => true,
        (None, None) => false,
        _ => {
            return Err(SshError::new(
                SshErrorKind::Configuration,
                "SOCKS5 authentication requires both username and password",
            ));
        }
    };
    let greeting: &[u8] = if has_credentials {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    stream.write_all(greeting).await.map_err(io_error)?;
    let mut selection = [0_u8; 2];
    stream.read_exact(&mut selection).await.map_err(io_error)?;
    if selection[0] != 5 {
        return Err(SshError::new(
            SshErrorKind::Proxy,
            "SOCKS5 proxy returned an invalid protocol version",
        ));
    }
    match selection[1] {
        0 => {}
        2 if has_credentials => {
            authenticate_socks5(
                &mut stream,
                username.expect("validated username"),
                password.expect("validated password"),
            )
            .await?;
        }
        0xff => {
            return Err(SshError::new(
                SshErrorKind::ProxyAuthentication,
                "SOCKS5 proxy accepted no offered authentication method",
            ));
        }
        _ => {
            return Err(SshError::new(
                SshErrorKind::ProxyAuthentication,
                "SOCKS5 proxy selected an unsupported authentication method",
            ));
        }
    }

    let host = endpoint.host.as_bytes();
    let host_length = u8::try_from(host.len()).map_err(|_| {
        SshError::new(
            SshErrorKind::Configuration,
            "SOCKS5 target hostname exceeds 255 bytes",
        )
    })?;
    let mut request = Vec::with_capacity(host.len() + 7);
    request.extend_from_slice(&[5, 1, 0, 3, host_length]);
    request.extend_from_slice(host);
    request.extend_from_slice(&endpoint.port.to_be_bytes());
    stream.write_all(&request).await.map_err(io_error)?;

    let mut response = [0_u8; 4];
    stream.read_exact(&mut response).await.map_err(io_error)?;
    if response[0] != 5 {
        return Err(SshError::new(
            SshErrorKind::Proxy,
            "SOCKS5 proxy returned an invalid protocol version",
        ));
    }
    if response[1] != 0 {
        return Err(SshError::new(
            SshErrorKind::Proxy,
            format!("SOCKS5 proxy rejected the target with code {}", response[1]),
        ));
    }
    match response[3] {
        1 => discard(&mut stream, 4 + 2).await?,
        3 => {
            let mut length = [0_u8; 1];
            stream.read_exact(&mut length).await.map_err(io_error)?;
            discard(&mut stream, usize::from(length[0]) + 2).await?;
        }
        4 => discard(&mut stream, 16 + 2).await?,
        _ => {
            return Err(SshError::new(
                SshErrorKind::Proxy,
                "SOCKS5 proxy returned an invalid address type",
            ));
        }
    }
    Ok(stream)
}

async fn authenticate_socks5(
    stream: &mut TcpStream,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    let username_length = u8::try_from(username.len()).map_err(|_| {
        SshError::new(
            SshErrorKind::Configuration,
            "SOCKS5 username exceeds 255 bytes",
        )
    })?;
    let password_length = u8::try_from(password.len()).map_err(|_| {
        SshError::new(
            SshErrorKind::Configuration,
            "SOCKS5 password exceeds 255 bytes",
        )
    })?;
    let mut request = Vec::with_capacity(username.len() + password.len() + 3);
    request.extend_from_slice(&[1, username_length]);
    request.extend_from_slice(username.as_bytes());
    request.push(password_length);
    request.extend_from_slice(password.as_bytes());
    stream.write_all(&request).await.map_err(io_error)?;
    let mut response = [0_u8; 2];
    stream.read_exact(&mut response).await.map_err(io_error)?;
    if response != [1, 0] {
        return Err(SshError::new(
            SshErrorKind::ProxyAuthentication,
            "SOCKS5 proxy rejected authentication",
        ));
    }
    Ok(())
}

async fn discard(stream: &mut TcpStream, length: usize) -> Result<(), SshError> {
    let mut remaining = vec![0_u8; length];
    stream.read_exact(&mut remaining).await.map_err(io_error)?;
    Ok(())
}

async fn open_proxy_command(command: &str) -> Result<ProxyCommandStream, SshError> {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    } else {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut process = Command::new(shell);
        process.arg("-c").arg(command);
        process
    };
    process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process.spawn().map_err(|error| {
        SshError::new(
            SshErrorKind::Proxy,
            format!("failed to start ProxyCommand: {error}"),
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| SshError::new(SshErrorKind::Proxy, "ProxyCommand stdin is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SshError::new(SshErrorKind::Proxy, "ProxyCommand stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SshError::new(SshErrorKind::Proxy, "ProxyCommand stderr is unavailable"))?;
    let stderr_task = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let remaining = MAX_PROXY_COMMAND_STDERR_BYTES.saturating_sub(captured.len());
                    captured.extend_from_slice(&buffer[..read.min(remaining)]);
                }
            }
        }
        captured
    });
    Ok(ProxyCommandStream {
        child,
        stdin,
        stdout,
        stderr_task,
    })
}

struct ProxyCommandStream {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr_task: JoinHandle<Vec<u8>>,
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(context, buffer)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.stdin).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_shutdown(context)
    }
}

impl Drop for ProxyCommandStream {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        self.stderr_task.abort();
    }
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn io_error(error: io::Error) -> SshError {
    SshError::new(SshErrorKind::Proxy, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;
    use tokio::net::TcpListener;

    fn endpoint() -> ConnectionProfile {
        ConnectionProfile::new(
            "target",
            "target-alias",
            "unresolved.internal",
            2222,
            "alice",
        )
    }

    #[tokio::test]
    async fn http_connect_uses_basic_authentication_and_returns_tunnel_stream() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("CONNECT unresolved.internal:2222 HTTP/1.1\r\n"));
            assert!(request.contains("Proxy-Authorization: Basic YWxpY2U6cHJveHktcGFzcw==\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut marker = [0_u8; 4];
            stream.read_exact(&mut marker).await.unwrap();
            assert_eq!(&marker, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });

        let mut stream = open_http_connect(
            "127.0.0.1",
            address.port(),
            Some("alice"),
            Some("proxy-pass"),
            &endpoint(),
        )
        .await
        .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut response = [0_u8; 4];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_authenticates_and_leaves_target_dns_resolution_to_proxy() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0_u8; 4];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 2, 0, 2]);
            stream.write_all(&[5, 2]).await.unwrap();

            let mut auth = vec![0_u8; 2 + 5 + 1 + 10];
            stream.read_exact(&mut auth).await.unwrap();
            assert_eq!(&auth, b"\x01\x05alice\x0aproxy-pass");
            stream.write_all(&[1, 0]).await.unwrap();

            let mut request_header = [0_u8; 5];
            stream.read_exact(&mut request_header).await.unwrap();
            assert_eq!(&request_header[..4], &[5, 1, 0, 3]);
            let mut destination = vec![0_u8; usize::from(request_header[4]) + 2];
            stream.read_exact(&mut destination).await.unwrap();
            assert_eq!(
                &destination[..destination.len() - 2],
                b"unresolved.internal"
            );
            assert_eq!(
                &destination[destination.len() - 2..],
                &2222_u16.to_be_bytes()
            );
            stream
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                .await
                .unwrap();
            let mut marker = [0_u8; 4];
            stream.read_exact(&mut marker).await.unwrap();
            assert_eq!(&marker, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });

        let mut stream = open_socks5(
            "127.0.0.1",
            address.port(),
            Some("alice"),
            Some("proxy-pass"),
            &endpoint(),
        )
        .await
        .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut response = [0_u8; 4];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn proxy_command_refuses_changed_target_parameters() {
        let proxy = RuntimeProxy::proxy_command(
            SecretString::new("helper %h %p".into()),
            Some("outdated".into()),
        );

        let error = match open_initial_stream(Some(&proxy), &endpoint()).await {
            Ok(_) => panic!("an unapproved command must not be started"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), SshErrorKind::ProxyCommandApproval);
        assert_eq!(error.stage(), Some(&ConnectionStage::Proxy));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn proxy_command_uses_stdio_as_a_bidirectional_stream() {
        let endpoint = endpoint();
        let command = "cat";
        let digest = proxy_command_approval_digest(command, &endpoint);
        let proxy = RuntimeProxy::proxy_command(SecretString::new(command.into()), Some(digest));
        let mut stream = open_initial_stream(Some(&proxy), &endpoint).await.unwrap();

        stream.write_all(b"round-trip\n").await.unwrap();
        stream.flush().await.unwrap();
        let mut response = [0_u8; 11];
        stream.read_exact(&mut response).await.unwrap();

        assert_eq!(&response, b"round-trip\n");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn proxy_command_uses_stdio_as_a_bidirectional_stream() {
        let endpoint = endpoint();
        // `more` is available with Windows and copies piped stdin to stdout.
        let command = "more";
        let digest = proxy_command_approval_digest(command, &endpoint);
        let proxy = RuntimeProxy::proxy_command(SecretString::new(command.into()), Some(digest));
        let mut stream = open_initial_stream(Some(&proxy), &endpoint).await.unwrap();

        stream.write_all(b"round-trip\r\n").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();

        assert!(
            String::from_utf8_lossy(&response).contains("round-trip"),
            "Windows ProxyCommand did not copy stdin to stdout"
        );
    }
}
