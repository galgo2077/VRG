//! Ephemeral, loopback-only HTTP adapter. This module never opens VrgEngine.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    error::{VrgError, VrgResult},
    view_data::{GraphQuery, ViewCatalog, ViewError},
};
use serde_json::{Value, json};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";

/// Error prefixes are private, bounded stage codes, consumed by the CLI exit mapper.
fn stage_error(code: &str) -> VrgError {
    let action = match code {
        "PROFILE_NOT_FOUND" => {
            "No hay perfil autorizado para esta ruta. Seleccione un proyecto registrado o indique --config; abrir el visor no ejecuta setup."
        }
        "PROFILE_AMBIGUOUS" => "Varios perfiles coinciden. Seleccione uno mediante --config.",
        "SCOPE_MISMATCH" | "ROOT_REVOKED" | "UNAUTHORIZED" => {
            "La ruta y el perfil no autorizan esta lectura. Seleccione el ámbito autorizado."
        }
        "STORE_MISSING" => {
            "El perfil no tiene un índice existente. La indexación es una operación separada del visor."
        }
        "SCHEMA_UNSUPPORTED" => {
            "El índice no es compatible con este visor; no se migró ni modificó."
        }
        "BIND_FAILED" => {
            "No se pudo abrir el puerto local. Use --port 0 o un puerto libre; no se detuvo otro proceso."
        }
        "SERVER_FAILED" | "RANDOM_FAILED" => "No se pudo iniciar el servidor local seguro.",
        _ => "Revise la ruta/perfil seleccionado; el visor no modifica las fuentes.",
    };
    VrgError::InvalidConfig(format!("VIEW:{code}: {action}"))
}
fn data_error(error: ViewError) -> VrgError {
    stage_error(error.code)
}

/// Exit mapping applies only to errors from the new view command.
pub fn exit_code(error: &VrgError) -> Option<i32> {
    let VrgError::InvalidConfig(message) = error else {
        return None;
    };
    let code = message.strip_prefix("VIEW:")?.split(':').next()?;
    Some(match code {
        "PROFILE_NOT_FOUND" | "PROFILE_AMBIGUOUS" | "ROOT_REVOKED" | "SCOPE_MISMATCH"
        | "UNAUTHORIZED" => 3,
        "STORE_MISSING" | "STORE_BUSY" | "SCHEMA_UNSUPPORTED" | "SOURCE_UNAVAILABLE"
        | "SNAPSHOT_EXPIRED" => 4,
        "BIND_FAILED" | "SERVER_FAILED" | "RANDOM_FAILED" => 5,
        _ => 2,
    })
}

pub async fn run(
    path: PathBuf,
    config: Option<PathBuf>,
    no_open: bool,
    port: u16,
) -> VrgResult<()> {
    // Resolution happens before binding and does not initialize storage.
    let catalog = Arc::new(
        ViewCatalog::resolve(&path, config.as_deref()).map_err(|error| {
            eprintln!("Ruta solicitada: {}", path.display());
            data_error(error)
        })?,
    );
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .map_err(|_| stage_error("BIND_FAILED"))?;
    listener
        .set_nonblocking(true)
        .map_err(|_| stage_error("SERVER_FAILED"))?;
    let port = listener
        .local_addr()
        .map_err(|_| stage_error("SERVER_FAILED"))?
        .port();
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://{host}");
    let mut random = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|_| stage_error("RANDOM_FAILED"))?;
    let token = hex::encode(random);
    let url = format!("{origin}/#token={token}");
    let running = Arc::new(AtomicBool::new(true));
    let server_running = running.clone();
    let server_host = host.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let started = Instant::now();
        // Keep status, inspector and static assets responsive while a complete
        // graph is being prepared. Work is bounded to four native threads;
        // there is no background daemon, extra listener or unbounded queue.
        let mut requests: Vec<thread::JoinHandle<()>> = Vec::new();
        // The socket is bound and all embedded routes are ready before this signal.
        let _ = ready_tx.send(());
        while server_running.load(Ordering::Acquire) && started.elapsed() < SESSION_LIFETIME {
            let mut index = 0;
            while index < requests.len() {
                if requests[index].is_finished() {
                    let _ = requests.swap_remove(index).join();
                } else {
                    index += 1;
                }
            }
            match listener.accept() {
                Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    if requests.len() >= 4 {
                        let _ = write_response(&mut stream, error_response(503, "SERVER_BUSY"));
                        continue;
                    }
                    let catalog = catalog.clone();
                    let host = server_host.clone();
                    let origin = origin.clone();
                    let token = token.clone();
                    let active = server_running.clone();
                    requests.push(thread::spawn(move || {
                        let response = match read_request(&mut stream) {
                            Ok(request) => respond(&catalog, &request, &host, &origin, &token),
                            Err(code) => error_response(400, code),
                        };
                        if active.load(Ordering::Acquire) {
                            // Always close the connection; no request pipelining.
                            let _ = write_response(&mut stream, response);
                        }
                    }));
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(15))
                }
                Err(_) => {
                    server_running.store(false, Ordering::Release);
                    break;
                }
            }
        }
        server_running.store(false, Ordering::Release);
        catalog.cancel();
        for request in requests {
            let _ = request.join();
        }
    });
    let health = ready_rx.recv_timeout(Duration::from_secs(2)).is_ok() && health_ready(port, &host);
    if !health {
        running.store(false, Ordering::Release);
        let _ = worker.join();
        return Err(stage_error("SERVER_FAILED"));
    }
    println!("Visor VRG local · SOLO LECTURA · sesión de hasta 8 horas");
    println!("Enlace privado de sesión; no compartir: {url}");
    println!("Ctrl+C cierra el servidor y revoca la sesión.");
    std::io::stdout().flush()?;
    let opener = if no_open {
        None
    } else {
        let mut command = Command::new("xdg-open");
        command
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(child) => Some(child),
            Err(_) => {
                eprintln!("No se pudo abrir el navegador. Abra el enlace local mostrado arriba.");
                None
            }
        }
    };
    let opener_running = running.clone();
    let opener_monitor = opener.map(|mut child| thread::spawn(move || {
        while opener_running.load(Ordering::Acquire) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() { eprintln!("El navegador no abrió automáticamente. Abra el enlace local mostrado arriba."); }
                    return;
                },
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(_) => { eprintln!("No se pudo comprobar la apertura del navegador; use el enlace local."); return; },
            }
        }
        // xdg-open may exec a browser. Leave that process to the user instead of
        // killing a browser when the ephemeral HTTP server closes.
    }));
    // A blocking waiter uses no long-lived SQLite transaction or storage lease.
    // Periodically notice expiry/server failure without requiring a second signal.
    let monitor = running.clone();
    let finished = tokio::task::spawn_blocking(move || {
        while monitor.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(100));
        }
    });
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            if signal.is_err() { eprintln!("No se pudo recibir Ctrl+C; cerrando el visor."); }
        },
        _ = finished => { eprintln!("La sesión local terminó o caducó."); }
    }
    running.store(false, Ordering::Release);
    worker.join().map_err(|_| stage_error("SERVER_FAILED"))?;
    if let Some(monitor) = opener_monitor {
        let _ = monitor.join();
    }
    Ok(())
}

fn health_ready(port: u16, host: &str) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let Ok(mut stream) = TcpStream::connect_timeout(&address.into(), Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    if write!(stream, "GET /health HTTP/1.1\r\nHost: {host}\r\n\r\n").is_err() {
        return false;
    }
    let mut response = Vec::new();
    stream.take(8192).read_to_end(&mut response).is_ok()
        && response.starts_with(b"HTTP/1.1 200 OK\r\n")
}

#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
}

trait DeadlineRead: Read {
    fn set_remaining_timeout(&self, remaining: Duration) -> std::io::Result<()>;
}
impl DeadlineRead for TcpStream {
    fn set_remaining_timeout(&self, remaining: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(remaining))
    }
}
#[cfg(test)]
impl<T: AsRef<[u8]>> DeadlineRead for std::io::Cursor<T> {
    fn set_remaining_timeout(&self, _: Duration) -> std::io::Result<()> {
        Ok(())
    }
}
fn read_request(stream: &mut impl DeadlineRead) -> Result<Request, &'static str> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    let started = Instant::now();
    loop {
        let remaining = Duration::from_secs(2)
            .checked_sub(started.elapsed())
            .filter(|value| !value.is_zero())
            .ok_or("REQUEST_TIMEOUT")?;
        stream
            .set_remaining_timeout(remaining)
            .map_err(|_| "INVALID_REQUEST")?;
        let count = stream.read(&mut chunk).map_err(|_| "INVALID_REQUEST")?;
        if count == 0 {
            return Err("INVALID_REQUEST");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_HEADER_BYTES {
            return Err("LIMIT_EXCEEDED");
        }
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            // Reject bodies and pipelined requests rather than ignoring trailing bytes.
            if bytes.len() != end + 4 {
                return Err("INVALID_REQUEST");
            }
            let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "INVALID_REQUEST")?;
            return parse_request(header);
        }
    }
}

fn parse_request(header: &str) -> Result<Request, &'static str> {
    let mut lines = header.split("\r\n");
    let mut first = lines.next().ok_or("INVALID_REQUEST")?.split(' ');
    let method = first.next().ok_or("INVALID_REQUEST")?;
    let target = first.next().ok_or("INVALID_REQUEST")?;
    if first.next() != Some("HTTP/1.1")
        || first.next().is_some()
        || !target.starts_with('/')
        || target.starts_with("//")
        || target.contains('#')
        || target.bytes().any(|b| b <= 32 || b == 127)
    {
        return Err("INVALID_REQUEST");
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("INVALID_REQUEST")?;
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 && b != b'\t' || b == 127)
        {
            return Err("INVALID_REQUEST");
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_string())
            .is_some()
        {
            return Err("INVALID_REQUEST");
        }
    }
    if headers.contains_key("transfer-encoding")
        || headers.get("content-length").is_some_and(|v| v != "0")
    {
        return Err("INVALID_REQUEST");
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
    })
}

struct Response {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
}
fn error_response(status: u16, code: &str) -> Response {
    json_response(status, json!({"api_version":"1", "error":{"code":code}}))
}
fn json_response(status: u16, value: Value) -> Response {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    if body.len() > MAX_RESPONSE_BYTES {
        return error_response(413, "LIMIT_EXCEEDED");
    }
    Response {
        status,
        mime: "application/json; charset=utf-8",
        body,
    }
}
fn static_response(mime: &'static str, bytes: &[u8]) -> Response {
    Response {
        status: 200,
        mime,
        body: bytes.to_vec(),
    }
}

fn secure_request(
    request: &Request,
    host: &str,
    origin: &str,
    token: &str,
) -> Result<(), (u16, &'static str)> {
    if request.headers.get("host").map(String::as_str) != Some(host) {
        return Err((403, "HOST_REJECTED"));
    }
    if request
        .headers
        .get("origin")
        .is_some_and(|value| value != origin)
        || request
            .headers
            .get("sec-fetch-site")
            .is_some_and(|value| value != "same-origin" && value != "none")
    {
        return Err((403, "ORIGIN_REJECTED"));
    }
    if request.method != "GET" {
        return Err((405, "METHOD_NOT_ALLOWED"));
    }
    if request.target.starts_with("/api/") {
        let expected = format!("Bearer {token}");
        let supplied = request
            .headers
            .get("authorization")
            .map(String::as_bytes)
            .unwrap_or_default();
        // Fixed-size token; do not early-return on a matching prefix.
        let mut difference = supplied.len() ^ expected.len();
        for index in 0..expected.len() {
            difference |= usize::from(
                expected.as_bytes()[index] ^ supplied.get(index).copied().unwrap_or_default(),
            );
        }
        if difference != 0 {
            return Err((401, "UNAUTHORIZED"));
        }
    }
    Ok(())
}

fn respond(
    catalog: &ViewCatalog,
    request: &Request,
    host: &str,
    origin: &str,
    token: &str,
) -> Response {
    if let Err((status, code)) = secure_request(request, host, origin, token) {
        return error_response(status, code);
    }
    let (path, raw_query) = request
        .target
        .split_once('?')
        .unwrap_or((&request.target, ""));
    if path.contains('%') || path.contains("..") || path.contains('\\') {
        return error_response(400, "INVALID_REQUEST");
    }
    match path {
        "/" => {
            return static_response(
                "text/html; charset=utf-8",
                include_bytes!("view_assets/index.html"),
            );
        }
        "/assets/app.js" => {
            return static_response(
                "text/javascript; charset=utf-8",
                include_bytes!("view_assets/app.js"),
            );
        }
        "/assets/nuclei.js" => {
            return static_response(
                "text/javascript; charset=utf-8",
                include_bytes!("view_assets/nuclei.js"),
            );
        }
        "/assets/style.css" => {
            return static_response(
                "text/css; charset=utf-8",
                include_bytes!("view_assets/style.css"),
            );
        }
        "/favicon.ico" => {
            return Response {
                status: 204,
                mime: "image/x-icon",
                body: Vec::new(),
            };
        }
        "/health" => return json_response(200, json!({"ready":true})),
        "/api/v1/session" => {
            return json_response(
                200,
                json!({"api_version":"1", "selected_project_id":catalog.selected_project_id(), "read_only":true, "local":true, "expires_in_seconds":SESSION_LIFETIME.as_secs()}),
            );
        }
        "/api/v1/projects" => return json_response(200, catalog.projects()),
        _ => (),
    }
    let query = match decode_query(raw_query) {
        Ok(value) => value,
        Err(code) => return error_response(400, code),
    };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 6 || segments[1..4] != ["api", "v1", "projects"] {
        return error_response(404, "NOT_FOUND");
    }
    let project_id = segments[4];
    let limit = match number(
        &query,
        "limit",
        if segments[5] == "nodes" {
            16 * 1024
        } else {
            500
        },
    ) {
        Ok(value) => value,
        Err(code) => return error_response(400, code),
    };
    let graph_query = GraphQuery {
        snapshot_id: query.get("snapshot_id").cloned(),
        cursor: query.get("cursor").cloned(),
        query: query.get("q").cloned(),
        limit,
        mode: query
            .get("mode")
            .cloned()
            .unwrap_or_else(|| "structural".into()),
    };
    let result = match segments[5] {
        "status" if segments.len() == 6 => catalog.status(project_id),
        "graph" | "search" if segments.len() == 6 => catalog.graph(project_id, &graph_query),
        "nodes" if segments.len() == 7 => {
            let Some(snapshot) = query.get("snapshot_id") else {
                return error_response(400, "SNAPSHOT_REQUIRED");
            };
            let offset = match number(&query, "offset", 0) {
                Ok(value) => value,
                Err(code) => return error_response(400, code),
            };
            catalog.detail(
                project_id,
                segments[6],
                snapshot,
                offset,
                limit,
                query.get("source").is_some_and(|v| v == "1"),
            )
        }
        "neighbors" if segments.len() == 7 => {
            let Some(snapshot) = query.get("snapshot_id") else {
                return error_response(400, "SNAPSHOT_REQUIRED");
            };
            catalog.neighbors(project_id, segments[6], snapshot)
        }
        _ => return error_response(404, "NOT_FOUND"),
    };
    match result {
        Ok(value) => json_response(200, value),
        Err(error) => error_response(
            match error.code {
                "UNAUTHORIZED" => 401,
                "ROOT_REVOKED" | "SCOPE_MISMATCH" => 403,
                "SNAPSHOT_EXPIRED" | "SOURCE_CHANGED" => 410,
                "STORE_BUSY" => 503,
                "LIMIT_EXCEEDED" => 413,
                "PROFILE_NOT_FOUND" | "NODE_NOT_FOUND" | "SOURCE_UNAVAILABLE" => 404,
                _ => 400,
            },
            error.code,
        ),
    }
}

fn number(
    query: &BTreeMap<String, String>,
    key: &str,
    default: usize,
) -> Result<usize, &'static str> {
    match query.get(key) {
        Some(value) => value.parse().map_err(|_| "INVALID_REQUEST"),
        None => Ok(default),
    }
}
fn decode_query(raw: &str) -> Result<BTreeMap<String, String>, &'static str> {
    let mut query = BTreeMap::new();
    if raw.len() > 8192 {
        return Err("LIMIT_EXCEEDED");
    }
    if raw.is_empty() {
        return Ok(query);
    }
    for pair in raw.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key)?;
        let value = decode_component(value)?;
        if query.insert(key, value).is_some() {
            return Err("INVALID_REQUEST");
        }
    }
    if query.get("q").is_some_and(|v| v.len() > 4096) {
        return Err("LIMIT_EXCEEDED");
    }
    Ok(query)
}
fn decode_component(value: &str) -> Result<String, &'static str> {
    let mut result = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'%' => {
                let high = bytes
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or("INVALID_REQUEST")?;
                let low = bytes
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or("INVALID_REQUEST")?;
                let decoded = (high * 16 + low) as u8;
                if decoded == 0 {
                    return Err("INVALID_REQUEST");
                }
                result.push(decoded);
            }
            b'+' => result.push(b' '),
            _ => result.push(byte),
        }
    }
    String::from_utf8(result).map_err(|_| "INVALID_REQUEST")
}
fn write_response(stream: &mut impl Write, response: Response) -> std::io::Result<()> {
    let reason = match response.status {
        204 => "No Content",
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        413 => "Content Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Security-Policy: {}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Frame-Options: DENY\r\n\r\n",
        response.status,
        reason,
        response.mime,
        response.body.len(),
        CSP
    )?;
    stream.write_all(&response.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(headers: &str) -> Request {
        parse_request(
            format!("GET /api/v1/projects HTTP/1.1\r\nHost: 127.0.0.1:4321\r\n{headers}")
                .trim_end(),
        )
        .unwrap()
    }
    #[test]
    fn authentication_host_origin_and_methods() {
        let valid = request("Authorization: Bearer secret");
        assert!(
            secure_request(&valid, "127.0.0.1:4321", "http://127.0.0.1:4321", "secret").is_ok()
        );
        for supplied in [
            "",
            "Authorization: Bearer wrong",
            "Authorization: Bearer secretlong",
        ] {
            assert_eq!(
                secure_request(
                    &request(supplied),
                    "127.0.0.1:4321",
                    "http://127.0.0.1:4321",
                    "secret"
                ),
                Err((401, "UNAUTHORIZED"))
            );
        }
        for supplied in [
            "Origin: null",
            "Origin: https://hostile.invalid",
            "Sec-Fetch-Site: cross-site",
            "Sec-Fetch-Site: same-site",
        ] {
            assert_eq!(
                secure_request(
                    &request(&format!("Authorization: Bearer secret\r\n{supplied}")),
                    "127.0.0.1:4321",
                    "http://127.0.0.1:4321",
                    "secret"
                ),
                Err((403, "ORIGIN_REJECTED"))
            );
        }
        assert_eq!(
            secure_request(
                &valid,
                "evil.invalid:4321",
                "http://127.0.0.1:4321",
                "secret"
            ),
            Err((403, "HOST_REJECTED"))
        );
        let mut post = valid;
        post.method = "POST".into();
        assert_eq!(
            secure_request(&post, "127.0.0.1:4321", "http://127.0.0.1:4321", "secret"),
            Err((405, "METHOD_NOT_ALLOWED"))
        );
    }
    #[test]
    fn malformed_http_and_bodies_are_rejected() {
        for raw in [
            "GET / HTTP/1.1\r\nHost: a\r\nHost: b",
            "GET http://host/ HTTP/1.1\r\nHost: a",
            "GET / HTTP/1.0\r\nHost: a",
            "GET / HTTP/1.1\r\nHost: a\r\nContent-Length: 1",
            "GET / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked",
        ] {
            assert!(parse_request(raw).is_err(), "{raw}");
        }
        let mut pipeline =
            std::io::Cursor::new(b"GET / HTTP/1.1\r\nHost: a\r\n\r\nGET / HTTP/1.1\r\n\r\n");
        assert!(read_request(&mut pipeline).is_err());
    }
    #[test]
    fn drip_fed_headers_have_absolute_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let reader = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let started = Instant::now();
            assert!(read_request(&mut stream).is_err());
            started.elapsed()
        });
        let mut stream = TcpStream::connect(address).unwrap();
        let writer = thread::spawn(move || {
            // Each byte is well within the per-read timeout. Only the absolute
            // header deadline stops this otherwise unbounded request.
            for _ in 0..26 {
                if stream.write_all(b"G").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        let elapsed = reader.join().unwrap();
        writer.join().unwrap();
        assert!(elapsed >= Duration::from_millis(1800));
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }
    #[test]
    fn query_unicode_and_bounds() {
        assert_eq!(
            decode_query("q=Espa%C3%B1ol+%F0%9F%8C%B2").unwrap()["q"],
            "Español 🌲"
        );
        for raw in ["q=%00", "q=%FF", "q=%x", "q=a&q=b"] {
            assert!(decode_query(raw).is_err());
        }
        assert!(decode_query(&format!("q={}", "x".repeat(4097))).is_err());
    }
    #[test]
    fn security_headers_and_response_budget() {
        let mut output = Vec::new();
        write_response(&mut output, error_response(401, "UNAUTHORIZED")).unwrap();
        let output = String::from_utf8(output).unwrap();
        for header in [
            "Cache-Control: no-store",
            "frame-ancestors 'none'",
            "Referrer-Policy: no-referrer",
            "X-Content-Type-Options: nosniff",
            "Cross-Origin-Resource-Policy: same-origin",
        ] {
            assert!(output.contains(header));
        }
        assert!(!output.contains("Access-Control-Allow-Origin"));
        assert!(!output.contains("unsafe-inline"));
        assert_eq!(
            json_response(200, json!({"text":"x".repeat(MAX_RESPONSE_BYTES)})).status,
            413
        );
    }
}
