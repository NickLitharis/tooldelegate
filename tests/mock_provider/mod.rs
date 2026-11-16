//! Minimal HTTP/SSE provider double shared by connector and pilot tests.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
};

pub struct Mock {
    pub status: &'static str,
    pub headers: &'static str,
    pub body: String,
}

pub fn read_request(stream: &mut TcpStream) -> String {
    let mut raw = Vec::new();
    let mut buffer = [0; 4096];
    let header_end = loop {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "connection closed before headers");
        raw.extend_from_slice(&buffer[..n]);
        if let Some(position) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while raw.len() < header_end + length {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "connection closed before body");
        raw.extend_from_slice(&buffer[..n]);
    }
    let body = String::from_utf8_lossy(&raw[header_end..header_end + length]).to_string();
    format!("{headers}{body}")
}

pub fn respond(mut stream: TcpStream, status: &str, headers: &str, body: &str) {
    write!(
        stream,
        "{status}Content-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
    .unwrap();
}

pub fn serve(mock: Mock) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let received = read_request(&mut stream);
        respond(stream, mock.status, mock.headers, &mock.body);
        received
    });
    (port, handle)
}

/// Serves one SSE body per connection, in order, until the queue is exhausted.
pub fn serve_sequence(bodies: Vec<String>) -> (u16, JoinHandle<Vec<String>>) {
    serve_mocks(
        bodies
            .into_iter()
            .map(|body| Mock {
                status: "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n",
                headers: "",
                body,
            })
            .collect(),
    )
}

/// Serves one arbitrary HTTP response per connection, in order.
pub fn serve_mocks(mocks: Vec<Mock>) -> (u16, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let mut received = Vec::new();
        for mock in mocks {
            let (mut stream, _) = listener.accept().unwrap();
            received.push(read_request(&mut stream));
            respond(stream, mock.status, mock.headers, &mock.body);
        }
        received
    });
    (port, handle)
}

pub fn sse(events: &[Value]) -> Mock {
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    Mock {
        status: "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n",
        headers: "",
        body,
    }
}

/// One SSE body whose single text delta is `text`, followed by provider-reported usage.
pub fn action_stream(text: &str) -> String {
    let events = [
        json!({"type": "response.output_text.delta", "delta": text}),
        json!({"type": "response.completed", "response": {
            "id": "resp_test", "model": "gpt-6-luna", "status": "completed",
            "usage": {"input_tokens": 100, "output_tokens": 10, "input_tokens_details": {"cached_tokens": 0}},
            "output": []
        }}),
    ];
    events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect()
}
