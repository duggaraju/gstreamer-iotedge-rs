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

use gst::prelude::*;
use gst_rtsp_server::prelude::*;
use gstreamer_app::{AppSrc, AppSink};
use gst::format::{GenericFormattedValue};
use gst::{Format, Bin, Element, Sample};
use gstreamer_base::prelude::{BaseSrcExt};
use gst::gst_element_error;

use azure_iot_rs::message::{IotHubMessage};
use azure_iot_rs::module::{IotHubModuleClient, IotHubModuleEvent};

use serde_json::{Value};

use std::sync::mpsc::{SyncSender, sync_channel};
use std::str;
use std::result::Result;
use std::process;
use serde_derive::{Deserialize, Serialize};

use tokio::task;
use futures::{join, StreamExt, SinkExt};

use anyhow::{Error};
use warp::Filter;
use warp::ws::Message;


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

#[derive(Clone)]
struct WebRtcContext {
    sender: SyncSender<Message>,
    webrtcbin: Element,
}

// JSON messages we communicate with
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum JsonMsg {
    Ice {
        candidate: String,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: u32,
    },
    Sdp {
        #[serde(rename = "type")]
        type_: String,
        sdp: String,
    },
}

#[derive(Debug, Clone)]
struct ModuleSettings
{
    pipeline: Option<String>,
    rtsp_pipeline: Option<String>,
    webrtc_pipeline: Option<String>,
    http_server: bool
}

#[derive(Clone)]
struct ModuleContext {
    settings: Option<ModuleSettings>,
    pipeline: Option<Element>,
    videosink: Option<Element>,
    audiosink: Option<Element>,
    webrtc_pipeline: Option<String>,
}

#[derive(Clone, Debug)]
struct RtspContext {
    audiosrc: Option<Element>,
    videosrc: Option<Element>,
    audiosink: Option<Element>,
    videosink: Option<Element>
}

impl ModuleSettings {
    fn from_json(settings: Value) -> Self {
        let desired = settings["desired"].as_object().expect("Expected desired settings!");
        ModuleSettings {
            pipeline: desired.get("pipeline").and_then(|val| val.as_str()).map(String::from),
            rtsp_pipeline: desired.get("rtsp_pipeline").and_then(|val| val.as_str()).map(String::from),
            webrtc_pipeline: desired.get("webrtc_pipeline").and_then(|val| val.as_str()).map(String::from),
            http_server: desired.get("http_server").and_then(|val| val.as_bool()).unwrap_or(true)
        }
    }

    fn from_env() -> Self {
        ModuleSettings {
            pipeline: std::env::var("pipeline").ok(),
            rtsp_pipeline: std::env::var("rtsp_pipeline").ok(),
            webrtc_pipeline: std::env::var("webrtc_pipeline").ok(),
            http_server: true
        }
    }
}

impl ModuleContext {
    fn new() -> Self {
        ModuleContext {
            settings: None,
            pipeline: None,
            videosink: None,
            audiosink: None,
            webrtc_pipeline: None
        }        
    }

    fn rtsp_context(&self) -> RtspContext {
        RtspContext {
            audiosink: self.audiosink.clone(),
            videosink: self.videosink.clone(),
            audiosrc: None,
            videosrc: None
        }
    }

    fn close_pipeline(&mut self) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_state(gst::State::Null).expect("failed to set pipeline to null");
            self.pipeline = None;
        }
    }

    fn create_pipeline(&mut self, pipeline: &str) {
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
    
        let bin = pipeline.as_ref().unwrap().downcast_ref::<Bin>().unwrap();
        self.videosink = bin.get_by_name_recurse_up("video");
        self.audiosink = bin.get_by_name_recurse_up("audio");
     
        pipeline.as_ref().unwrap()
            .set_state(gst::State::Playing)
            .expect("Unable to set the pipeline to the `Playing` state");
        self.pipeline = pipeline.ok();
    
        self.process_pipeline();
    } 
    
    fn process_pipeline(&self) {
        let pipeline = self.pipeline.as_ref().unwrap().clone();
        let bus = pipeline.get_bus().expect("Failed to get the bus from pipeline!");
        bus.add_watch(move |_, msg| {
            use gst::MessageView;
            let cont = match msg.view() {
                MessageView::Eos(..) => {
                    error!("Received EOS on the pipeline!"); 
                    pipeline.set_state(gst::State::Null)
                    .expect("Unable to set the pipeline to the `Null` state");
                    false
                },
                MessageView::Error(err) => {
                    error!(
                        "Error from {:?}: {} ({:?})",
                        err.get_src().map(|s| s.get_path_string()),
                        err.get_error(),
                        err.get_debug()
                    );
                    false
                }
                _ => true,
            };
            glib::Continue(cont)
        }).expect("Failed to add watch!");    
    }

    fn update(&mut self, settings: ModuleSettings) {
        if let Some(ref pipeline) = settings.pipeline {
            self.create_pipeline(pipeline);
        }

        if let Some(ref pipeline) = settings.rtsp_pipeline {
            let base_context = self.rtsp_context();
            base_context.rtsp_server(pipeline);
        }

        let use_http = settings.http_server;
        let pipeline = settings.webrtc_pipeline.as_ref().map_or(String::new(), String::from);
        self.settings = Some(settings);
        if use_http {
            let _server = task::spawn( async move { http_server(pipeline).await; });
        }    
    }
}

impl RtspContext {
    fn push_buffer_modify_time(source: &AppSrc, sample: Sample) {
        let buffer = sample.get_buffer();
        let segment = sample.get_segment().unwrap();
    
        if let Some(ref buffer) = buffer {
            let opts = buffer.get_pts();
            let mut pts = opts;
            /* Convert the PTS/DTS to running time so they start from 0 */
            if pts.is_some() {
                info!("Found buffer {}", pts);
                let value = segment.to_running_time(pts);
                if let GenericFormattedValue::Time(value) = value {
                    pts = value;
                }
            }
    
            let odts = buffer.get_dts();
            let mut dts = odts;
            if dts.is_some() {
                let value = segment.to_running_time(dts);
                if let GenericFormattedValue::Time(value) = value {
                    dts = value;
                }
            }
    
            /* Make writable so we can adjust the timestamps */
            let mut copy = buffer.copy();
            let buffer_ref = copy.make_mut();
            buffer_ref.set_pts(pts);
            buffer_ref.set_dts(dts);
            let result  = source.push_buffer(copy);
            match result {
                Ok(_) => { info!("successfully pushed buffer {} {} {} {}", opts,  odts, pts, dts); },
                Err(err) => { error!("Failed to push buffer {} {} {}", err, pts, dts); }
            }
        }
    }
    
    fn push_buffer(source: &AppSrc, sample: Sample) {
        let buffer = sample.get_buffer_owned();
        if let Some(buffer) = buffer {
            let pts = buffer.get_pts();
            let dts = buffer.get_dts();
            let result  = source.push_buffer(buffer);
            match result {
                Ok(_) => { info!("successfully pushed buffer {} {}", pts, dts); },
                Err(err) => { error!("Failed to push buffer {} {} {}", err, pts, dts); }
            };
        }
    }
    
    fn on_need_data(&self, source: &AppSrc, audio: bool) {
        let sink =  if audio { 
            self.audiosink.as_ref().unwrap()
        } else {
            self.videosink.as_ref().unwrap()
        };
    
        let appsink = sink.downcast_ref::<AppSink>().unwrap();
        let sample = appsink.pull_sample();
        if let Ok(sample) = sample {
            RtspContext::push_buffer(source, sample);
        }
    }
    
    fn connect_sink_source(&self, sink: &AppSink, source: &AppSrc) {
        source.set_format(Format::Time);
        let pads = sink.get_sink_pads();
        for pad in &pads {
            let caps = pad.get_current_caps();
            info!("Found caps for sink {}", caps.as_ref().unwrap());
            source.set_caps(caps.as_ref());
        }
        let context = self.clone();
        source.connect_need_data(move |source, _| { context.on_need_data(source, false); });
        source.connect_enough_data(|_| {
            error!("Enough data stop sending!!!");
        });
    }
    
    fn on_media_configure(&self, _: &gst_rtsp_server::RTSPMediaFactory, media: &gst_rtsp_server::RTSPMedia) {
    
        let mut context = self.clone();
        let element = media.get_element().unwrap();
        let bin = element.downcast::<Bin>().unwrap();

        context.videosrc = bin.get_by_name_recurse_up("videosrc");
        context.audiosrc = bin.get_by_name_recurse_up("audiosrc");    

        if let Some(ref video) = self.videosrc {
            let videosrc = video.downcast_ref::<AppSrc>().unwrap();
            if let Some(ref videosink) = context.videosink {
                context.connect_sink_source(videosink.downcast_ref::<AppSink>().unwrap(), &videosrc);
            }    
        }

        if let Some(ref audio) = context.audiosrc {
            let audiosrc = audio.downcast_ref::<AppSrc>().unwrap();
            if let Some(ref audiosink) = context.audiosink {
                context.connect_sink_source(audiosink.downcast_ref::<AppSink>().unwrap(), &audiosrc);
            }    
        }
    }
    
    fn rtsp_server(&self, pipeline: &str) {
        let rtsp_pipeline = shellexpand::env(pipeline).unwrap();
        info!("expanded RTSP pipeline: {}", rtsp_pipeline);
    
        let server = gst_rtsp_server::RTSPServer::new();
        let mounts = server.get_mount_points().unwrap();
        let factory = gst_rtsp_server::RTSPMediaFactory::new();
        factory.set_launch(&rtsp_pipeline);
        factory.set_shared(true);
        let context = self.clone();
        factory.connect_media_configure(move |factory, media| { context.on_media_configure(factory, media); });
        factory.connect_media_constructed(|_, _| { 
            info!("constructed media !!");
        });
        // Now we add a new mount-point and tell the RTSP server to serve the content
        // provided by the factory we configured above, when a client connects to
        // this specific path.
        mounts.add_factory("/player", &factory);
    
        let _id = server.attach(None);
    
        let address = server.get_address().unwrap();
        info!("Stream ready at rtsp://{}:{}/player", address.as_str(), server.get_bound_port());    
    }    
}

impl WebRtcContext {
        
    fn start_webrtc_pipeline(pipeline: String, sender: SyncSender<Message>) -> anyhow::Result<WebRtcContext> {

        info!("expanding environemnt variables in pipeline: {}", pipeline);
        let expanded_pipeline = shellexpand::env(&pipeline).unwrap();
        let mut context = gst::ParseContext::new();
        let element = gst::parse_launch_full(&expanded_pipeline, Some(&mut context), gst::ParseFlags::empty());
        if let Err(err) = element {
            if let Some(gst::ParseError::NoSuchElement) = err.kind::<gst::ParseError>() {
                error!("Missing element(s): {:?}", context.get_missing_elements());
            } else {
                error!("Failed to parse pipeline: {}", err);
            }
            return Err(anyhow!("Failed to created webrtc pipeline!"));
        }
    
        let element = element.unwrap();
        let bin = element.downcast_ref::<Bin>().expect("Element must be a bin!");
     
        let webrtcbin = bin.get_by_name_recurse_up("webrtcbin").expect("missing element by name webrtcbin");
        let client = WebRtcContext {
            sender: sender,
            webrtcbin: webrtcbin.clone() 
        };
    
        let client2 = client.clone();
        webrtcbin.connect("on-negotiation-needed", false, move |values| {
            let _webrtc = values[0].get::<gst::Element>().unwrap();
            if let Err(err) = client2.on_negotiation_needed() {
                gst_element_error!(
                    _webrtc.unwrap(),
                    gst::LibraryError::Failed,
                    ("Failed to negotiate: {:?}", err)
                );
            }
    
            None
        }).unwrap();
    
        let client3 = client.clone();
        webrtcbin
        .connect("on-ice-candidate", false, move |values| {
            let _webrtc = values[0].get::<gst::Element>().expect("Invalid argument");
            let mlineindex = values[1].get_some::<u32>().expect("Invalid argument");
            let candidate = values[2]
                .get::<String>()
                .expect("Invalid argument")
                .unwrap();
            if let Err(err) = client3.on_ice_candidate(mlineindex, candidate) {
                gst_element_error!(
                    _webrtc.unwrap(),
                    gst::LibraryError::Failed,
                    ("Failed to send ICE candidate: {:?}", err)
                );
            }
            None
        }).unwrap();  
    
        element
            .set_state(gst::State::Playing)
            .expect("Unable to set the pipeline to the `Playing` state");
    
        Ok(client)
    }
    
    fn on_offer_created(&self, reply: Result<Option<&gst::StructureRef>, gst::PromiseError>)-> anyhow::Result<()> {
    
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                bail!("Offer creation future got no reponse: {:?}", err);
            }
        };
    
        let offer = reply.unwrap()
            .get_value("offer")
            .unwrap()
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .expect("Invalid argument")
            .unwrap();
        self.webrtcbin
            .emit("set-local-description", &[&offer, &None::<gst::Promise>])
            .unwrap();
    
        info!(
            "sending SDP offer to peer: {}",
            offer.get_sdp().as_text().unwrap()
        );
    
        let message = serde_json::to_string(&JsonMsg::Sdp {
            type_: "offer".to_string(),
            sdp: offer.get_sdp().as_text().unwrap(),
        })
        .unwrap();
    
        self.send_text_message(&message); 
        Ok(())
    }
    
    fn on_negotiation_needed(&self) -> Result<(), failure::Error> {
        info!("Creating negotiation offer");
    
        let client = self.clone();
        let promise = gst::promise::Promise::with_change_func(move |reply| {
    
            if let Err(err) = client.on_offer_created(reply) {
                gst_element_error!(
                    client.webrtcbin,
                    gst::LibraryError::Failed,
                    ("Failed to send SDP offer: {:?}", err)
                );
            }
        });
    
        self.webrtcbin
            .emit("create-offer", &[&None::<gst::Structure>, &promise])
            .unwrap();
    
        Ok(())
    }
    
    fn on_ice_candidate(&self, mlineindex: u32, candidate: String) -> Result<(), failure::Error> {
        let message = serde_json::to_string(&JsonMsg::Ice {
            candidate,
            sdp_mline_index: mlineindex,
        })
        .unwrap();
    
        self.send_text_message(&message);
        Ok(())
    }
    
    fn send_text_message(&self, message: &str) {
        let text_message = Message::text(message);
        self.send_message(text_message);
    }
    
    fn send_message(&self , message: Message) {
        self.sender.send(message).unwrap();
    }
    
    fn process_message(&self, message: Message) -> anyhow::Result<()> {
        let msg = message.to_str().unwrap();
        if msg.starts_with("ERROR") {
            bail!("Got error message: {}", msg);
        }
    
        let json_msg: JsonMsg = serde_json::from_str(msg)?;
    
        match json_msg {
            JsonMsg::Sdp { type_, sdp } => self.handle_sdp(&type_, &sdp),
            JsonMsg::Ice { sdp_mline_index, candidate } => self.handle_ice(sdp_mline_index, &candidate)
        }
    }
    
    fn handle_ice(&self, sdp_mline_index: u32, candidate: &str) -> Result<(), anyhow::Error> {
        self.webrtcbin
            .emit("add-ice-candidate", &[&sdp_mline_index, &candidate])
            .unwrap();   
        Ok(())
    }
    
    fn on_answer_created(&self,  reply: Result<Option<&gst::StructureRef>, gst::PromiseError>) -> Result<(), anyhow::Error> {
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                bail!("Answer creation future got no reponse: {:?}", err);
            }
        };
    
        let answer = reply.unwrap()
            .get_value("answer")
            .unwrap()
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .expect("Invalid argument")
            .unwrap();
        self.webrtcbin
            .emit("set-local-description", &[&answer, &None::<gst::Promise>])
            .unwrap();
    
        println!(
            "sending SDP answer to peer: {}",
            answer.get_sdp().as_text().unwrap()
        );
    
        let message = serde_json::to_string(&JsonMsg::Sdp {
            type_: "answer".to_string(),
            sdp: answer.get_sdp().as_text().unwrap(),
        })
        .unwrap();
    
        self.send_text_message(&message);   
        Ok(())
    }
    
    fn handle_sdp(&self, type_: &str, sdp: &str) -> Result<(), Error> {
        if type_ == "answer" {
            info!("Received answer: {}\n", sdp);
    
            let ret = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
                .map_err(|_| anyhow!("Failed to parse SDP answer"))?;
            let answer =
                gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Answer, ret);
    
            self.webrtcbin
                .emit("set-remote-description", &[&answer, &None::<gst::Promise>])
                .unwrap();
    
            Ok(())
        } else if type_ == "offer" {
            print!("Received offer:\n{}\n", sdp);
    
            let ret = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
                .map_err(|_| anyhow!("Failed to parse SDP offer"))?;
    
            // And then asynchronously start our pipeline and do the next steps. The
            // pipeline needs to be started before we can create an answer
            let clone = self.clone();
            self.webrtcbin.call_async(move |_pipeline| {
    
                let offer = gst_webrtc::WebRTCSessionDescription::new(
                    gst_webrtc::WebRTCSDPType::Offer,
                    ret,
                );
    
                clone
                    .webrtcbin
                    .emit("set-remote-description", &[&offer, &None::<gst::Promise>])
                    .unwrap();
    
                let app_clone = clone.clone();
                let promise = gst::Promise::with_change_func(move |reply| {
    
                    if let Err(err) = app_clone.on_answer_created(reply) {
                        gst_element_error!(
                            app_clone.webrtcbin,
                            gst::LibraryError::Failed,
                            ("Failed to send SDP answer: {:?}", err)
                        );
                    }
                });
    
                clone
                    .webrtcbin
                    .emit("create-answer", &[&None::<gst::Structure>, &promise])
                    .unwrap();
            });
    
            Ok(())
        } else {
            bail!("Sdp type is not \"answer\" but \"{}\"", type_)
        }
    }    
}

fn module_callback(context: &mut ModuleContext, event: IotHubModuleEvent) {
    let _result = match event {
        IotHubModuleEvent::Message(message) => handle_message(message),
        IotHubModuleEvent::Twin(settings) => handle_twin(context, settings)
    };
}

fn handle_message(message: IotHubMessage) -> Result<(), &'static str> {
    info!("Received hub message {}", message.content_type());
    return Ok(())
}

fn handle_twin(context: &mut ModuleContext, settings: Value) -> Result<(), &'static str> {

    let desired = ModuleSettings::from_json(settings);
    context.update(desired);
    Ok(())
}

fn handle_iot() {
    let mut context = ModuleContext::new();
    let mut client = IotHubModuleClient::new(|event| { module_callback(&mut context, event); });
    info!("Initialized client");
    client.do_work();
}

async fn handle_webscoket(pipeline: String, socket: warp::ws::WebSocket) {

    info!("Staring a web socket session for pipeline: {}", pipeline);
    // let pipeline = String::new();
    let (mut writer, mut reader) = socket.split();
    let (sender, receiver) = sync_channel::<Message>(1);

    let send = task::spawn(async move {
        let webrtc_client = WebRtcContext::start_webrtc_pipeline(pipeline, sender).unwrap();
        loop {
            let next = reader.next().await;
    
            if let None = next {
                break;
            }
            let msg = next.unwrap().ok();
    
            let msg = match msg {
                Some(msg) => msg,
                None => {
                    break;
                }
            };
    
            if msg.is_text() {
                info!("received text {}", msg.to_str().unwrap());
                webrtc_client.process_message(msg).unwrap();
            }
        };    
    });

    task::yield_now().await;

    loop {
        let message = receiver.recv();
        if let Err(e) = message {
            error!("Error sending message {}", e);
            break;
        }
        writer.send(message.unwrap()).await.unwrap();
    }

    send.await.unwrap();
}

async fn http_server(webrtc_pipeline: String) {

    info!("Initializing HTTP server...");

    let root = warp::path("wwwroot")
        .and(warp::fs::dir("wwwroot"));    

    let media_path = std::env::var("media_root").unwrap_or(String::from("/tmp"));
    let media = warp::path("media")
    .and(warp::fs::dir(media_path));    

    let use_ws = !webrtc_pipeline.is_empty();

    // If web rtc is enabled allow web sockets.
    let ws = warp::path("ws")
    .and_then( move || async move {
        if use_ws {
            Ok(true)
        } else {
            Err(warp::reject::not_found())            
        }
    })
    .and(warp::ws())
    .map(move |_, ws: warp::ws::Ws| {
        let pipeline = webrtc_pipeline.clone();
        ws.on_upgrade( |ws| async move { handle_webscoket(pipeline, ws).await; })
    });

    info!("started http server....");
    let routes = warp::get().and(root.or(media).or(ws));
    warp::serve(routes)
    .run(([0, 0, 0, 0], 8000)).await;
}

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

    let iot = task::spawn_blocking(handle_iot);
    main_loop.run();
    iot.await.unwrap();
}
