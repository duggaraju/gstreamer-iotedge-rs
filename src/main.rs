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

use crate::webrtc::http_server;
use crate::settings::ModuleSettings;
use crate::rtsp::RtspContext;
use crate::iot::{ModuleContext};
use std::result::Result;
use std::process;

use tokio::task;

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
async fn main() {

    env_logger::from_env(env_logger::Env::default().default_filter_or("info")).init();
    ctrlc::set_handler(move || {
        info!("Terminating on Ctrl+C");
        process::exit(-1);
    }).expect("Error setting Ctrl-C handler");

    //ensure media root is always set.
    if let None = std::env::var_os("media_root") {
        std::env::set_var("media_root", "/tmp");
    }

    gst::init().unwrap();
    plugin_register_static().unwrap();
    let main_loop = glib::MainLoop::new(None, false);
    info!("Initialized gstreamer");

    // local testing only.
    // let settings = ModuleSettings::from_env();
    // let mut context = ModuleContext::new();
    // context.update(settings);

    let iot = task::spawn_blocking(|| { ModuleContext::handle_iot(); });
    main_loop.run();
    iot.await.unwrap();
}
