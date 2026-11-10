use std::sync::mpsc::{self, Receiver};

use log::{debug, warn};

/// Login1 `PrepareForSleep` watcher. Runs on a dedicated thread with a blocking
/// D-Bus connection; the daemon receives only boolean transitions.
pub struct SleepWatcher {
    receiver: Receiver<bool>,
}

impl SleepWatcher {
    pub fn start() -> Option<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("ember-logind".into())
            .spawn(move || {
                let connection = match zbus::blocking::Connection::system() {
                    Ok(connection) => connection,
                    Err(error) => {
                        debug!("logind unavailable: {error}");
                        return;
                    }
                };
                let proxy = match zbus::blocking::Proxy::new(
                    &connection,
                    "org.freedesktop.login1",
                    "/org/freedesktop/login1",
                    "org.freedesktop.login1.Manager",
                ) {
                    Ok(proxy) => proxy,
                    Err(error) => {
                        debug!("logind manager proxy failed: {error}");
                        return;
                    }
                };
                let signals = match proxy.receive_signal("PrepareForSleep") {
                    Ok(signals) => signals,
                    Err(error) => {
                        warn!("cannot subscribe to PrepareForSleep: {error}");
                        return;
                    }
                };
                for message in signals {
                    let preparing: bool = message.body().deserialize().unwrap_or(false);
                    if sender.send(preparing).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { receiver })
    }

    /// Returns `true` for "preparing to sleep", `false` for "resumed".
    pub fn drain(&self) -> Vec<bool> {
        let mut out = Vec::new();
        while let Ok(value) = self.receiver.try_recv() {
            out.push(value);
        }
        out
    }
}

/// Mapping used by the coordinator and its tests.
pub fn is_sleep(preparing: bool) -> bool {
    preparing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_explicit() {
        assert!(is_sleep(true));
        assert!(!is_sleep(false));
    }
}
