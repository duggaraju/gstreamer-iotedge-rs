#[macro_use]
extern crate log as logger;
#[macro_use]
extern crate gst;
#[macro_use]
extern crate glib;
#[macro_use]
extern crate std;
#[macro_use]
extern crate anyhow;

extern crate serde;
extern crate serde_derive;
extern crate gstreamer_webrtc as gst_webrtc;
extern crate gstreamer_rtsp_server as gst_rtsp_server;
extern crate ctrlc;

mod plugins;
mod settings;
mod rtsp;
mod webrtc;
mod iot;
mod media;

use anyhow::{Result, Error};
use crate::settings::Settings;
use crate::iot::{IotModule};
use crate::media::{MediaPipeline};
use std::process;

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    plugins::register(plugin)?;
    Ok(())
}

gst_plugin_define!(
    test,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION")),
    "MIT/X11",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    "BUILD_REL_DATE"
);

#[tokio::main]
async fn main() -> Result<(), Error> {

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    //ensure media root is always set.
    if let None = std::env::var_os("media_root") {
        std::env::set_var("media_root", "/tmp");
    }

    gst::init()?;
    plugin_register_static()?;
    let main_loop = glib::MainLoop::new(None, false);
    info!("Initialized gstreamer");

    if let Some(_) = std::env::var_os("IOTEDGE_WORKLOADURI") {
        // We are running as an iot edge module.
        let _iot = IotModule::start();
    } else {
        // Run as a normal container..
        let settings = Settings::from_env();
        let mut pipeline = MediaPipeline::new();
        pipeline.update(settings);
    }
    ctrlc::set_handler(move || {
        info!("Terminating on Ctrl+C");
        process::exit(-1);
    }).expect("Error setting Ctrl-C handler");

    main_loop.run();
    Ok(())
}
