//! Prints what gilrs sees: the pads connected at start-up, then every
//! event for ten seconds. For finding out why a pad does nothing in
//! tuiba. Run with `cargo run -p tuiba --example padlog`.

use std::time::{Duration, Instant};

fn main() {
    let mut gilrs = match gilrs::Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(err) => {
            eprintln!("gilrs failed to start: {err}");
            return;
        }
    };
    for (id, pad) in gilrs.gamepads() {
        println!(
            "pad {id}: {:?} (mapping: {:?}, power: {:?})",
            pad.name(),
            pad.mapping_source(),
            pad.power_info()
        );
    }
    println!("press buttons for ten seconds...");
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        while let Some(event) = gilrs.next_event() {
            println!("{:?} {:?}", event.id, event.event);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
