mod receiver;
mod sender;
mod status;

use crate::config::{AppConfig, Role};
use crate::error::Result;

pub use receiver::ReceiverRuntime;
pub use sender::SenderRuntime;
pub use status::{ManagedService, ServiceStatus, StatusObserver};

pub fn start_configured_service(
    config: &AppConfig,
    observer: StatusObserver,
) -> Result<Box<dyn ManagedService>> {
    match config.role {
        Role::Sender => Ok(Box::new(SenderRuntime::start(config, observer)?)),
        Role::Receiver => Ok(Box::new(ReceiverRuntime::start(config, observer)?)),
    }
}

pub fn write_error_log(path: Option<&str>, message: &str) {
    let target = path.unwrap_or("key2pc-error.log");
    let _ = std::fs::write(target, format!("{message}\n"));
}

pub(crate) fn update_rtt_ewma(slot: &mut Option<f32>, sample_ms: f32) {
    *slot = Some(match *slot {
        Some(prev) => prev * 0.7 + sample_ms * 0.3,
        None => sample_ms,
    });
}

pub(crate) fn is_timeout_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}
