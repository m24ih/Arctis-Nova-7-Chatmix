mod audio;
mod controller;
mod device;
mod usb;

use anyhow::Result;
use log::info;

use controller::ArctisController;

fn main() -> Result<()> {
    env_logger::init();
    info!("Initializing Arctis Nova 7 ChatMix Controller...");

    let controller = ArctisController::new()?;
    controller.start()?;

    Ok(())
}
