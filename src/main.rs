//! mechfight/1 bot: JSON lines on stdin/stdout. Never prints anything else to stdout, never crashes.
use mechbot::bot;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;

fn send(line: &Value) {
    let mut out = io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, line);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn main() {
    // A reader thread lets the main loop see when several observations are queued.
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut r = io::BufReader::with_capacity(1 << 20, stdin.lock());
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut bot: Option<bot::Bot> = None;
    let mut last_move = json!([0.0, 0.0]);
    loop {
        let mut line = match rx.recv() {
            Ok(l) => l,
            Err(_) => break,
        };
        // Skip to the newest queued line, but never skip past a non-observation message.
        loop {
            if !line.contains("\"type\":\"obs\"") && !line.contains("\"type\": \"obs\"") {
                break;
            }
            match rx.try_recv() {
                Ok(next) => line = next,
                Err(_) => break,
            }
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match msg["type"].as_str() {
            Some("hello") => {
                let b = catch_unwind(AssertUnwindSafe(|| bot::Bot::new(&msg)));
                match b {
                    Ok(b) => bot = Some(b),
                    Err(_) => eprintln!("init panicked"),
                }
                send(&json!({"type": "ready", "name": "quietstorm"}));
            }
            Some("obs") => {
                let action = match bot.as_mut() {
                    Some(b) => catch_unwind(AssertUnwindSafe(|| b.decide(&msg))).ok(),
                    None => None,
                };
                match action {
                    Some(a) => {
                        last_move = a["move"].clone();
                        send(&a);
                    }
                    None => {
                        // Fallback: keep moving the way we were, toward safety on the next good tick.
                        send(&json!({"tick": msg["tick"], "move": last_move}));
                    }
                }
            }
            Some("end") => break,
            _ => {}
        }
    }
}
