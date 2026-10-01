//! Prints what the platform backend reports, for troubleshooting:
//! `cargo run --example list_interfaces`

use data_traffic_manager::{format, platform};

fn main() {
    let samples = match platform::collect() {
        Ok(samples) => samples,
        Err(err) => {
            eprintln!("collect failed: {err}");
            std::process::exit(1);
        }
    };
    println!("uptime: {:?}", platform::uptime());
    println!("{:<40} {:<9} {:<4} {:<7} {:>12} {:>12}  description", "key", "kind", "up", "visible", "rx", "tx");
    for s in samples {
        println!(
            "{:<40} {:<9} {:<4} {:<7} {:>12} {:>12}  {}",
            s.key,
            s.kind.id(),
            if s.is_up { "yes" } else { "no" },
            if s.visible { "yes" } else { "no" },
            format::bytes(s.rx_bytes),
            format::bytes(s.tx_bytes),
            s.description
        );
    }
}
