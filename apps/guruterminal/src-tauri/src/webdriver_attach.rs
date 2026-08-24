use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub const PORT_ENV_VAR: &str = "TAURI_WEBDRIVER_PORT";
pub const DEFAULT_PORT: u16 = 4445;

const BIND_WAIT: Duration = Duration::from_secs(15);
const STATUS_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortClass {
    Free,
    WebDriver,
    Foreign,
}

pub fn configured_port() -> u16 {
    std::env::var(PORT_ENV_VAR)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

pub fn wait_for_bind_port() {
    let port = configured_port();
    if let Err(error) = wait_until_port_bindable(port, BIND_WAIT) {
        eprintln!("Guru Terminal WebDriver cannot bind 127.0.0.1:{port}: {error}");
        std::process::exit(1);
    }
}

pub fn require_status_endpoint() {
    let port = configured_port();
    if let Err(error) = wait_for_status_document(port, STATUS_WAIT) {
        eprintln!("Guru Terminal WebDriver did not serve GET /status on 127.0.0.1:{port}: {error}");
        std::process::exit(1);
    }
}

fn wait_until_port_bindable(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match classify_port(port) {
            PortClass::Free => return Ok(()),
            PortClass::WebDriver if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            PortClass::WebDriver => {
                return Err(
                    "a previous WebDriver listener is still serving GET /status".to_string()
                );
            }
            PortClass::Foreign => {
                return Err(
                    "occupied by a non-WebDriver listener; set TAURI_WEBDRIVER_PORT to a free loopback port"
                        .to_string(),
                );
            }
        }
    }
}

fn wait_for_status_document(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut last = "connection refused".to_string();
    while Instant::now() < deadline {
        match fetch_status(port) {
            Ok(_) => return Ok(()),
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(last)
}

fn classify_port(port: u16) -> PortClass {
    if fetch_status(port).is_ok() {
        return PortClass::WebDriver;
    }
    if tcp_connects(port) {
        return PortClass::Foreign;
    }
    PortClass::Free
}

fn tcp_connects(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(150),
    )
    .is_ok()
}

fn fetch_status(port: u16) -> Result<serde_json::Value, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(400)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(400)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(b"GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| error.to_string())?;
    let mut body = String::new();
    stream
        .read_to_string(&mut body)
        .map_err(|error| error.to_string())?;
    let document = body
        .split("\r\n\r\n")
        .nth(1)
        .ok_or_else(|| "WebDriver /status response had no body".to_string())?;
    let value: serde_json::Value = serde_json::from_str(document.trim())
        .map_err(|error| format!("WebDriver /status was not JSON: {error}"))?;
    if value.get("value").is_none() {
        return Err("WebDriver /status JSON is missing value".to_string());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn classify_port_treats_closed_port_as_free() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let port = listener.local_addr().expect("listener address").port();
        drop(listener);
        assert_eq!(classify_port(port), PortClass::Free);
    }

    #[test]
    fn classify_port_treats_tcp_accept_without_status_as_foreign() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let port = listener.local_addr().expect("listener address").port();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                drop(stream);
            }
        });
        assert_eq!(classify_port(port), PortClass::Foreign);
    }

    #[test]
    fn fetch_status_requires_webdriver_value_document() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let port = listener.local_addr().expect("listener address").port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("status client");
            let mut buf = [0_u8; 256];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"value\":{\"ready\":false}}\n",
                )
                .expect("status response");
        });
        let status = fetch_status(port).expect("webdriver status JSON");
        assert_eq!(status["value"]["ready"], false);
        handle.join().expect("status thread");
    }
}
