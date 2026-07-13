use super::STREAM_KEEPALIVE_PONG_ID;
use crate::protocol::ServerEvent;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

fn stream_keepalive_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_secs(30)
    }
}

pub(super) fn stream_keepalive_ticker() -> time::Interval {
    let interval = stream_keepalive_interval();
    let mut ticker = time::interval_at(time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

/// Ticker driving the sleep-assertion heartbeat while a tool call is executing.
///
/// The assertion carries a bounded TTL (~150s) and only really re-arms once ~90s
/// have elapsed, so a 30s tick is enough to keep a legitimately long tool (a
/// 40-minute build) from letting the machine sleep mid-work, while costing one
/// cheap no-op call per tick otherwise.
///
/// Note this deliberately treats "the tool task is still alive" as progress: at
/// this layer a slow tool and a hung tool are indistinguishable. The wedge this
/// guards against (a provider stream that stops producing chunks) never reaches
/// these ticks, so it still expires.
pub(super) fn power_heartbeat_ticker() -> time::Interval {
    let interval = Duration::from_secs(30);
    let mut ticker = time::interval_at(time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

pub(super) fn send_stream_keepalive_mpsc(event_tx: &mpsc::UnboundedSender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
    });
}
