use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winping::{Buffer, Pinger};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// Round-trip time in milliseconds.
    Reply(u32),
    /// No reply within the timeout, or the request could not be sent.
    Timeout,
    /// Target name could not be resolved — shown separately from a timeout.
    Unresolved,
}

/// A sample plus the unix time (seconds) it was taken at.
pub type Stamped = (Sample, i64);

/// Spawns the ping loop on its own thread and returns the sample stream.
/// The thread ends when the receiver is dropped (i.e. when the app exits, or
/// the settings change the target and a new loop replaces this one).
///
/// It deliberately does NOT wake the UI through egui (`request_repaint` from
/// another thread can be lost and freeze eframe 0.29's loop); the heartbeat
/// in `win.rs` drives frames and the UI polls this channel.
pub fn spawn(target: String, interval_ms: u64, timeout_ms: u32) -> Receiver<Stamped> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pinglive-ping".into())
        .spawn(move || {
            let mut pinger = match Pinger::new() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("pinglive: cannot create ICMP handle: {e}");
                    return;
                }
            };
            pinger.set_timeout(timeout_ms);

            let interval = Duration::from_millis(interval_ms);
            let mut buffer = Buffer::new();
            let mut addr: Option<IpAddr> = None;
            let mut last_resolve = Instant::now() - Duration::from_secs(3600);

            loop {
                let started = Instant::now();

                // Resolve lazily, and retry at most every 15s while it fails,
                // so a DNS outage does not spin.
                if addr.is_none() && started.duration_since(last_resolve) >= Duration::from_secs(15)
                {
                    last_resolve = started;
                    addr = resolve(&target);
                }

                let sample = match addr {
                    None => Sample::Unresolved,
                    Some(ip) => match pinger.send(ip, &mut buffer) {
                        Ok(rtt) => Sample::Reply(rtt),
                        Err(_) => Sample::Timeout,
                    },
                };

                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                if tx.send((sample, at)).is_err() {
                    return; // app closed
                }

                let elapsed = started.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
            }
        })
        .expect("spawn ping thread");
    rx
}

fn resolve(target: &str) -> Option<IpAddr> {
    if let Ok(ip) = target.parse::<IpAddr>() {
        return Some(ip);
    }
    (target, 0u16)
        .to_socket_addrs()
        .ok()?
        .map(|sa| sa.ip())
        // Prefer IPv4: game servers and 8.8.8.8 style targets resolve faster
        // and ICMPv6 is more often filtered on home routers.
        .min_by_key(|ip| if ip.is_ipv4() { 0 } else { 1 })
}
