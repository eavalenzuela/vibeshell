mod audio;
mod battery;
mod network;

pub use audio::{AudioProvider, AudioStatus};
pub use battery::{BatteryProvider, BatteryStatus, BatteryTrend};
pub use network::{NetworkProvider, NetworkStatus};

pub trait StatusProvider {
    type Output;

    fn poll(&self) -> Option<Self::Output>;
}
