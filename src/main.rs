#[macro_use]
extern crate log as logger;
#[macro_use]
extern crate gstreamer as gst;
#[macro_use]
extern crate glib;
#[macro_use]
extern crate std;
#[macro_use]
extern crate lazy_static;

extern crate gstreamer_rtsp_server as gst_rtsp_server;
extern crate azure_iot_rs_sys as azure;
mod plugins;

use gst::prelude::*;
use gst_rtsp_server::prelude::*;
use azure::iothub::{IotHubMessage, IotHubModuleClient, IotHubModuleEvent};
use serde_json::{Value};
use std::str;
use std::path::Path;
use std::{convert::Infallible, net::SocketAddr};
use hyper::{Body, Request, Response, Server};
use hyper::header::{HeaderValue, ACCESS_CONTROL_ALLOW_ORIGIN};
use hyper::service::{make_service_fn, service_fn};
use tokio::task;
use futures::join;
use hyper_staticfile;
use std::result::{Result};
use gstreamer_app::{AppSrc, AppSink};
use gst::format::{GenericFormattedValue};
use gst::{Format, Bin, Element};
use std::sync::{Arc, Mutex};
use gstreamer_base::prelude::{BaseSrcExt};


fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    plugins::register(plugin)?;
    Ok(())
}


struct Context {
    pipeline: Option<Element>,
    videosink: Option<Element>,
    audiosink: Option<Element>,
}

lazy_static! {
    static ref CONTEXT: Arc<Mutex<Context>> = {
        Arc::new(Mutex::new(Context {
        pipeline: None,
        videosink: None,
        audiosink: None
        }))
    };    
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

async fn handle(request: Request<Body>) -> Result<Response<Body>, Infallible> {


    info!("Request: {} {}", request.method(), request.uri());

    // First, resolve the request. Returns a future for a `ResolveResult`.
    let root_path = std::env::var("media_root").unwrap_or(String::from("wwwroot"));
    let root = Path::new(&root_path);
    let result = hyper_staticfile::resolve(&root, &request)
        .await
        .unwrap();

    // Then, build a response based on the result.
    // The `ResponseBuilder` is typically a short-lived, per-request instance.
    let mut response = hyper_staticfile::ResponseBuilder::new()
        .request(&request)
        .cache_headers(Some(0))
        .build(result)
        .unwrap();

    response.headers_mut().insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));

    Ok(response)
}

fn module_callback(event: IotHubModuleEvent) {
    let _result = match event {
        IotHubModuleEvent::Message(message) => handle_message(message),
        IotHubModuleEvent::Twin(settings) => handle_twin(settings)
    };
}

fn handle_message(message: IotHubMessage) -> Result<(), &'static str> {
    info!("Received hub message {}", message.content_type());
    return Ok(())
}

fn handle_twin(settings: Value) -> Result<(), &'static str>{
    let desired = &settings["desired"]["pipeline"];
    let rtsp_pipeline = &settings["desired"]["rtsp_pipeline"];
    if let Value::String(desired) = desired {
        create_pipeline(desired);
        task::spawn_blocking(move || { process_pipeline(); });
        if let Value::String(rtsp_pipeline) = rtsp_pipeline {
            rtsp_server(rtsp_pipeline);
        }
    }
    Ok(())
}

fn create_pipeline(pipeline: &str) {
    info!("expanding environemnt variables in pipeline: {}", pipeline);
    let expanded_pipeline = shellexpand::env(pipeline).unwrap();
    let mut context = gst::ParseContext::new();
    let pipeline = gst::parse_launch_full(&expanded_pipeline, Some(&mut context), gst::ParseFlags::empty());
    if let Err(err) = pipeline {
        if let Some(gst::ParseError::NoSuchElement) = err.kind::<gst::ParseError>() {
            error!("Missing element(s): {:?}", context.get_missing_elements());
        } else {
            error!("Failed to parse pipeline: {}", err);
        }
        return;
    }

    pipeline.as_ref().unwrap()
        .set_state(gst::State::Playing)
        .expect("Unable to set the pipeline to the `Playing` state");

    let mut guard = CONTEXT.lock().unwrap();
    let bin = pipeline.as_ref().unwrap().downcast_ref::<Bin>().unwrap();
    (*guard).videosink = bin.get_by_name_recurse_up("video");
    (*guard).audiosink = bin.get_by_name_recurse_up("audio");
    (*guard).pipeline = pipeline.ok();
}

fn process_pipeline() {

    let guard = CONTEXT.lock().unwrap();
    let bus = (*guard).pipeline.as_ref().unwrap().get_bus().unwrap();
    drop(guard);

    for msg in bus.iter_timed(gst::CLOCK_TIME_NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                error!(
                    "Error from {:?}: {} ({:?})",
                    err.get_src().map(|s| s.get_path_string()),
                    err.get_error(),
                    err.get_debug()
                );
                break;
            }
            _ => (),
        }
    }

    let guard = CONTEXT.lock().unwrap();
    (*guard).pipeline.as_ref().unwrap()
        .set_state(gst::State::Null)
        .expect("Unable to set the pipeline to the `Null` state");
}

fn handle_iot() {
    let mut client = IotHubModuleClient::new(module_callback);
    info!("Initialized client");
    client.do_work();
}

fn on_need_data(source: &gstreamer_app::AppSrc, audio: bool) {

    let guard = CONTEXT.lock().unwrap();
    let sink =  if audio {
        (*guard).audiosink.as_ref().unwrap()
    } else {
        (*guard).videosink.as_ref().unwrap()
    };

    let appsink = sink.downcast_ref::<AppSink>().unwrap();
    let sample = appsink.pull_sample();

    if let Ok(sample) = sample {
        let buffer = sample.get_buffer();
        let segment = sample.get_segment().unwrap();

        if let Some(ref buffer) = buffer {
            let mut pts = buffer.get_pts();
            /* Convert the PTS/DTS to running time so they start from 0 */
            if pts.is_some() {
                let value = segment.to_running_time(pts);
                if let GenericFormattedValue::Time(value) = value {
                    pts = value;
                }
            }
    
            let mut dts = buffer.get_dts();
            if dts.is_some() {
                let value = segment.to_running_time(dts);
                if let GenericFormattedValue::Time(value) = value {
                    dts = value;
                }
            }
    
            /* Make writable so we can adjust the timestamps */
            let mut copy = buffer.copy();
            let copyref = copy.make_mut();
            copyref.set_pts(pts);
            copyref.set_dts(dts);
            let result  = source.push_buffer(copy);
            match result {
                Ok(_) => { debug!("successfully pushed buffer {} {}", pts, dts); },
                Err(err) => { error!("Failed to push buffer {} {} {}", err, pts, dts)}
            }
        }
    }
}

fn connect_sink_source(sink: &AppSink, source: &AppSrc) {
    source.set_format(Format::Time);
    let pads = sink.get_sink_pads();
    for pad in &pads {
        let caps = pad.get_current_caps();
        info!("Found caps for sink {}", caps.as_ref().unwrap());
        source.set_caps(caps.as_ref());
    }
    source.connect_need_data(|source, _| { on_need_data(source, false); });
    source.connect_enough_data(|_| {
        error!("Enough data stop sending!!!");
    });
}

fn on_media_configure(_: &gst_rtsp_server::RTSPMediaFactory, media: &gst_rtsp_server::RTSPMedia) {

    let guard = CONTEXT.lock().unwrap();

    if (*guard).pipeline.is_some() {

        let element = media.get_element().unwrap();
        let bin = element.downcast::<Bin>().unwrap();

        let video = bin.get_by_name_recurse_up("videosrc");
        if let Some(video) = video {
            let videosrc = video.downcast::<AppSrc>().unwrap();
            if let Some(ref videosink) = (*guard).videosink {
                connect_sink_source(videosink.downcast_ref::<AppSink>().unwrap(), &videosrc);
            }    
        }

        let audio = bin.get_by_name_recurse_up("audiosrc");    
        if let Some(audio) = audio {
            let audiosrc = audio.downcast::<AppSrc>().unwrap();
            if let Some(ref audiosink) = (*guard).audiosink {
                connect_sink_source(audiosink.downcast_ref::<AppSink>().unwrap(), &audiosrc);
            }    
        }
    }
}

fn rtsp_server(pipeline: &str) {
    let main_loop = glib::MainLoop::new(None, false);
    let server = gst_rtsp_server::RTSPServer::new();
    let mounts = server.get_mount_points().unwrap();
    let factory = gst_rtsp_server::RTSPMediaFactory::new();
    factory.set_launch(pipeline);
    factory.set_shared(true);
    factory.connect_media_configure(on_media_configure);
    factory.connect_media_constructed(|_, _| { 
        info!("constructed media !!");
    });
    // Now we add a new mount-point and tell the RTSP server to serve the content
    // provided by the factory we configured above, when a client connects to
    // this specific path.
    mounts.add_factory("/player", &factory);

    let id = server.attach(None);

    let address = server.get_address().unwrap();
    info!("Stream ready at rtsp://{}:{}/player", address.as_str(), server.get_bound_port());

    // Start the mainloop. From this point on, the server will start to serve
    // our quality content to connecting clients.
    main_loop.run();
    glib::source_remove(id);
}

async fn http_server() {
    info!("Initializing HTTP server...");
    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));

    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(handle))
    });

    let http_server = Server::bind(&addr).serve(make_svc);
    info!("started http server....");
    http_server.await.unwrap();
}

#[tokio::main]
async fn main() {

    env_logger::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gstreamer::init().unwrap();
    plugin_register_static().unwrap();
    info!("Initialized gstreamer");

    let server = http_server();
    let iot = task::spawn_blocking(move || { handle_iot(); });
    let _ = join!(server, iot);
}