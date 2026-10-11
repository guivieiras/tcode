//! A player-owned loopback endpoint. File bytes travel over the existing authenticated
//! query connection; dropping this owner cancels its listener and active reads.
use std::{io, path::PathBuf, sync::Arc};

use tcode_client::HostLink;
use tcode_protocol::{MAX_FILE_RANGE_BYTES, Query, QueryResponse};
use tokio::{
    io::{AsyncWriteExt as _, BufReader},
    net::TcpStream,
};

use crate::{
    http::{read_request, response},
    preview::{Task, into_tokio, serve_loopback},
};

pub struct FileStream {
    url: String,
    _listener: Task,
}

struct Source {
    host: HostLink,
    path: PathBuf,
    size: u64,
    mime: String,
    token: String,
}

impl FileStream {
    pub fn new(host: HostLink, path: PathBuf, size: u64, mime: String) -> io::Result<Self> {
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let url = format!("http://{address}/{token}");
        let listener = into_tokio(listener)?;
        let source = Arc::new(Source {
            host,
            path,
            size,
            mime,
            token,
        });
        let task = serve_loopback(listener, move |socket| {
            let source = source.clone();
            async move {
                let _ = serve(socket, source).await;
            }
        });
        Ok(Self {
            url,
            _listener: task,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

async fn serve(socket: TcpStream, source: Arc<Source>) -> io::Result<()> {
    let mut socket = BufReader::new(socket);
    let request = read_request(&mut socket).await?;
    if request.path != format!("/{}", source.token) || !request.body.is_empty() {
        return response(&mut socket, "404 Not Found", "text/plain", b"").await;
    }
    if !matches!(request.method.as_str(), "GET" | "HEAD") {
        return response(&mut socket, "405 Method Not Allowed", "text/plain", b"").await;
    }
    let range = request.headers.get("range");
    let Some((start, end)) = byte_range(range.map(String::as_str), source.size) else {
        return socket.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", source.size).as_bytes()).await;
    };
    let status = if range.is_some() {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let content_range = if range.is_some() {
        format!(
            "Content-Range: bytes {start}-{}/{size}\r\n",
            end - 1,
            size = source.size
        )
    } else {
        String::new()
    };
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{content_range}Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n", source.mime, end - start).as_bytes()).await?;
    if request.method == "HEAD" {
        return socket.flush().await;
    }
    let mut offset = start;
    while offset < end {
        let length = (end - offset).min(MAX_FILE_RANGE_BYTES as u64) as u32;
        let response = source
            .host
            .query(Query::ReadFileRange {
                path: source.path.clone(),
                offset,
                length,
                expected_size: source.size,
            })
            .await
            .map_err(|error| io::Error::other(error.message))?;
        let QueryResponse::FileBytes(bytes) = response else {
            return Err(io::Error::other("Unexpected file range response"));
        };
        if bytes.len() != length as usize {
            return Err(io::Error::other("Incomplete file range"));
        }
        socket.write_all(&bytes).await?;
        offset += bytes.len() as u64;
    }
    socket.flush().await
}

// End is exclusive. Multiple ranges are unnecessary for a sequential media player.
fn byte_range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    let Some(header) = header else {
        return Some((0, size));
    };
    let (start, end) = header.strip_prefix("bytes=")?.split_once('-')?;
    if size == 0 {
        return None;
    }
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        return (suffix > 0).then_some((size.saturating_sub(suffix), size));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        size
    } else {
        end.parse::<u64>().ok()?.saturating_add(1).min(size)
    };
    (start < end && start < size).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    #[test]
    fn player_seeks_read_exact_host_ranges_and_close_releases_listener() {
        let root = std::env::temp_dir().join(format!("tcode-stream-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("clip.mp4");
        std::fs::write(&path, b"0123456789").unwrap();
        let host = tcode_runtime::pipe::spawn_host(
            tcode_services::store::SessionStore::open_at(root.join("store")).unwrap(),
            tcode_runtime::pipe::HostServices::default(),
        )
        .unwrap();
        let stream = FileStream::new(host.link(), path, 10, "video/mp4".into()).unwrap();
        let url = url::Url::parse(stream.url()).unwrap();
        let address = format!("127.0.0.1:{}", url.port().unwrap());
        crate::block_on(async {
            for (range, status, content_range, body) in [
                ("bytes=3-6", "206 Partial Content", "bytes 3-6/10", "3456"),
                ("bytes=8-", "206 Partial Content", "bytes 8-9/10", "89"),
                ("bytes=-2", "206 Partial Content", "bytes 8-9/10", "89"),
                ("bytes=10-", "416 Range Not Satisfiable", "bytes */10", ""),
                (
                    "bytes=1-2,4-5",
                    "416 Range Not Satisfiable",
                    "bytes */10",
                    "",
                ),
            ] {
                let mut socket = TcpStream::connect(&address).await.unwrap();
                socket
                    .write_all(
                        format!(
                            "GET {} HTTP/1.1\r\nHost: localhost\r\nRange: {range}\r\n\r\n",
                            url.path()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut response = String::new();
                socket.read_to_string(&mut response).await.unwrap();
                let (head, actual) = response.split_once("\r\n\r\n").unwrap();
                assert!(head.starts_with(&format!("HTTP/1.1 {status}")));
                assert!(head.contains(&format!("Content-Range: {content_range}")));
                assert_eq!(actual, body);
            }
            for (method, path, status) in [
                ("HEAD", url.path(), "200 OK"),
                ("GET", "/wrong-token", "404 Not Found"),
            ] {
                let mut socket = TcpStream::connect(&address).await.unwrap();
                socket
                    .write_all(
                        format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut response = String::new();
                socket.read_to_string(&mut response).await.unwrap();
                assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
                assert_eq!(response.split_once("\r\n\r\n").unwrap().1, "");
            }
        });
        // Await cancellation so the assertion observes completed teardown.
        crate::block_on(async {
            stream._listener.cancel().await;
            assert!(TcpStream::connect(&address).await.is_err());
        });
        host.shutdown_blocking().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
