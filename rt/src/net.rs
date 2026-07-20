//! `net` module: a small HTTP/1.1 server and client, written from scratch on
//! std::net — zero dependencies. Designed for Plix backends:
//!
//! ```plix
//! net.serve("127.0.0.1:8080", func(req) {
//!     return net.response(200, "text/plain", "hello " + req["path"]);
//! });
//! ```

use crate::heap::*;
use crate::value::{to_display, Caller, OpResult};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn err<T>(m: impl Into<String>) -> OpResultT<T> {
    Err(m.into())
}
type OpResultT<T> = Result<T, String>;

fn want_str(v: V, name: &str) -> OpResultT<String> {
    unsafe {
        if is_ptr(v) {
            if let HeapObj::Str(s) = payload(v) {
                return Ok(s.clone());
            }
        }
        err(format!("{}: expected string, got {}", name, kind_name(v)))
    }
}

// ---------------------------------------------------------------------------
// server
// ---------------------------------------------------------------------------

struct Request {
    method: String,
    target: String,
    path: String,
    query: HashMap<String, String>,
    version: String,
    headers: HashMap<String, V>,
    body: Vec<u8>,
}

fn reason(code: i64) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        418 => "I'm a teapot",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

const MAX_HEADER: usize = 256 * 1024;

fn read_request(stream: &mut TcpStream) -> OpResultT<Option<Request>> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(60)))
        .ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end;
    loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = pos;
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                if buf.is_empty() {
                    return Ok(None);
                }
                return err("connection closed mid-request");
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEADER && find_subslice(&buf, b"\r\n\r\n").is_none() {
                    return err("request header too large");
                }
            }
            Err(e) => return err(format!("read: {}", e)),
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut body = buf[header_end + 4..].to_vec();

    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    if method.is_empty() {
        return err("malformed request line");
    }

    let mut headers: HashMap<String, V> = HashMap::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some(colon) = line.find(':') {
            let k = line[..colon].trim().to_lowercase();
            let v = line[colon + 1..].trim().to_string();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            headers.insert(k, mk_string(v));
        }
    }
    while body.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) => return err(format!("read body: {}", e)),
        }
    }
    body.truncate(content_length);

    let (path, query) = match target.find('?') {
        Some(q) => (
            target[..q].to_string(),
            parse_query(&target[q + 1..]),
        ),
        None => (target.clone(), HashMap::new()),
    };

    Ok(Some(Request {
        method,
        target,
        path,
        query,
        version,
        headers,
        body,
    }))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        match pair.find('=') {
            Some(eq) => {
                out.insert(url_decode(&pair[..eq]), url_decode(&pair[eq + 1..]));
            }
            None => {
                out.insert(url_decode(pair), String::new());
            }
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?');
                out.push(h);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// net.serve(addr, handler) — sequential HTTP server (v0.1). The handler is
/// called once per request with a request object and must return a response
/// object ({code, body, headers}), a string, or null (=204).
pub fn net_serve(caller: &mut dyn Caller, args: &[V]) -> OpResult {
    let addr = if args.is_empty() {
        return err("net.serve: expected (addr, handler)");
    } else {
        want_str(args[0], "net.serve")?
    };
    if args.len() < 2 {
        return err("net.serve: expected (addr, handler)");
    }
    let handler = args[1];
    let listener = TcpListener::bind(&addr).map_err(|e| format!("net.serve: bind {}: {}", addr, e))?;
    eprintln!("[plix net] listening on http://{}", addr);
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[plix net] accept error: {}", e);
                continue;
            }
        };
        if let Err(e) = handle_conn(caller, &mut stream, handler) {
            eprintln!("[plix net] connection error: {}", e);
        }
    }
    Ok(NULL)
}

fn handle_conn(caller: &mut dyn Caller, stream: &mut TcpStream, handler: V) -> OpResultT<()> {
    let req = match read_request(stream)? {
        Some(r) => r,
        None => return Ok(()),
    };

    // build the request object
    let mut keys = HashMap::new();
    keys.insert("method".to_string(), mk_string(req.method.clone()));
    keys.insert("path".to_string(), mk_string(req.path.clone()));
    keys.insert("target".to_string(), mk_string(req.target.clone()));
    keys.insert("version".to_string(), mk_string(req.version.clone()));
    let mut qm = HashMap::new();
    for (k, v) in req.query {
        qm.insert(k, mk_string(v));
    }
    keys.insert("query".to_string(), mk_map(qm));
    keys.insert("headers".to_string(), mk_map(req.headers));
    keys.insert(
        "body".to_string(),
        mk_string(String::from_utf8_lossy(&req.body).into_owned()),
    );
    let req_obj = mk_map(keys);

    let resp = match caller.call(handler, &[req_obj]) {
        Ok(r) => r,
        Err(e) => {
            write_response(
                stream,
                500,
                "text/plain; charset=utf-8",
                format!("handler error: {}", e).as_bytes(),
                &[],
            )?;
            return Ok(());
        }
    };

    // interpret the response value
    let (mut code, mut ctype, mut body_str, mut extra_headers): (
        i64,
        String,
        String,
        Vec<(String, String)>,
    ) = (200, "text/plain; charset=utf-8".to_string(), String::new(), Vec::new());
    unsafe {
        if is_null(resp) {
            code = 204;
        } else if is_ptr(resp) {
            match payload(resp) {
                HeapObj::Str(s) => {
                    body_str = s.clone();
                }
                HeapObj::Map(m) => {
                    if let Some(&c) = m.get("code") {
                        if is_int(c) {
                            code = as_int(c);
                        }
                    }
                    if let Some(&b) = m.get("body") {
                        body_str = to_display(b);
                    }
                    if let Some(&ct) = m.get("content_type") {
                        if let Some(s) = as_opt_str(ct) {
                            ctype = s;
                        }
                    }
                    if let Some(&hs) = m.get("headers") {
                        if is_ptr(hs) {
                            if let HeapObj::Map(hm) = payload(hs) {
                                for (k, &v) in hm.iter() {
                                    extra_headers.push((k.clone(), to_display(v)));
                                }
                            }
                        }
                    }
                }
                _ => {
                    body_str = to_display(resp);
                }
            }
        } else {
            body_str = to_display(resp);
        }
    }
    write_response(stream, code, &ctype, body_str.as_bytes(), &extra_headers)?;
    Ok(())
}

#[allow(static_mut_refs)]
unsafe fn as_opt_str(v: V) -> Option<String> {
    if !is_ptr(v) {
        return None;
    }
    match payload(v) {
        HeapObj::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn write_response(
    stream: &mut TcpStream,
    code: i64,
    ctype: &str,
    body: &[u8],
    extra: &[(String, String)],
) -> OpResultT<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        code,
        reason(code),
        ctype,
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write: {}", e))
}

/// net.response(code, body [, content_type]) -> response object
pub fn net_response(_caller: &mut dyn Caller, args: &[V]) -> OpResult {
    if args.len() < 2 {
        return err("net.response: expected (code, body [, content_type])");
    }
    let code = if is_int(args[0]) {
        as_int(args[0])
    } else {
        200
    };
    let body = to_display(args[1]);
    let ctype = if args.len() > 2 {
        to_display(args[2])
    } else {
        "text/plain; charset=utf-8".to_string()
    };
    let mut m = HashMap::new();
    m.insert("code".to_string(), mk_int(code));
    m.insert("body".to_string(), mk_string(body));
    m.insert("content_type".to_string(), mk_string(ctype));
    Ok(mk_map(m))
}

// ---------------------------------------------------------------------------
// client
// ---------------------------------------------------------------------------

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> OpResultT<Url> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("url must start with http:// (https not supported in v0.1): {}", url))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => (
            hostport[..i].to_string(),
            hostport[i + 1..].parse().map_err(|_| "bad port".to_string())?,
        ),
        None => (hostport.to_string(), 80),
    };
    Ok(Url {
        host,
        port,
        path: path.to_string(),
    })
}

fn http_request(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    ctype: Option<&str>,
) -> OpResultT<V> {
    let u = parse_url(url)?;
    let mut stream = TcpStream::connect((u.host.as_str(), u.port))
        .map_err(|e| format!("connect {}:{}: {}", u.host, u.port, e))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(60)))
        .ok();
    let body_bytes = body.unwrap_or(b"");
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: plix/0.2\r\nConnection: close\r\n",
        method, u.path, u.host
    );
    if !body_bytes.is_empty() {
        req.push_str(&format!(
            "Content-Type: {}\r\nContent-Length: {}\r\n",
            ctype.unwrap_or("text/plain"),
            body_bytes.len()
        ));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .and_then(|_| stream.write_all(body_bytes))
        .map_err(|e| format!("write: {}", e))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {}", e))?;
    let header_end = find_subslice(&raw, b"\r\n\r\n").unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..header_end.min(raw.len())]).into_owned();
    let body_start = (header_end + 4).min(raw.len());
    let body_raw = &raw[body_start..];

    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let code: i64 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some(colon) = line.find(':') {
            headers.insert(
                line[..colon].trim().to_lowercase(),
                mk_string(line[colon + 1..].trim().to_string()),
            );
        }
    }

    let mut m = HashMap::new();
    m.insert("code".to_string(), mk_int(code));
    m.insert(
        "body".to_string(),
        mk_string(String::from_utf8_lossy(body_raw).into_owned()),
    );
    m.insert("headers".to_string(), mk_map(headers));
    Ok(mk_map(m))
}

/// net.get(url) -> {code, body, headers}
pub fn net_get(_caller: &mut dyn Caller, args: &[V]) -> OpResult {
    if args.is_empty() {
        return err("net.get: expected (url)");
    }
    http_request("GET", &want_str(args[0], "net.get")?, None, None)
}

/// net.post(url, body [, content_type]) -> {code, body, headers}
pub fn net_post(_caller: &mut dyn Caller, args: &[V]) -> OpResult {
    if args.len() < 2 {
        return err("net.post: expected (url, body [, content_type])");
    }
    let body = to_display(args[1]);
    let ctype = if args.len() > 2 {
        Some(to_display(args[2]))
    } else {
        None
    };
    http_request(
        "POST",
        &want_str(args[0], "net.post")?,
        Some(body.as_bytes()),
        ctype.as_deref(),
    )
}
