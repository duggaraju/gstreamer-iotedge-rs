#[macro_use]
extern crate anyhow;

extern crate ctrlc;
extern crate gstreamer_rtsp_server as gst_rtsp_server;
extern crate gstreamer_webrtc as gst_webrtc;
extern crate serde;
extern crate serde_derive;

mod iot;
mod media;
mod plugins;
mod rtsp;
mod settings;
mod webrtc;

use crate::media::MediaPipeline;
use crate::settings::Settings;
use crate::{iot::IotModule, plugins::plugin_register_static};
use log::info;
use std::{env, process};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    //ensure media root is always set.
    if env::var_os("media_root").is_none() {
        env::set_var("media_root", "/tmp");
    }

    gstreamer::init()?;
    plugin_register_static()?;
    let main_loop = gstreamer::glib::MainLoop::new(None, false);
    info!("Initialized gstreamer");

    if env::var_os("IOTEDGE_WORKLOADURI").is_some() {
        // We are running as an iot edge module.
        let _iot = IotModule::start();
    } else {
        // Run as a normal container..
        let settings = Settings::from_env();
        let mut pipeline = MediaPipeline::new();
        pipeline.update(settings)?;
    }
    ctrlc::set_handler(|| {
        info!("Terminating on Ctrl+C");
        process::exit(-1);
    })?;

    main_loop.run();
    Ok(())
}
