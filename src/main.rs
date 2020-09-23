#[macro_use]
extern crate log as logger;
#[macro_use]
extern crate gst;
#[macro_use]
extern crate glib;
#[macro_use]
extern crate std;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate anyhow;

extern crate serde;
extern crate serde_derive;
extern crate gstreamer_webrtc as gst_webrtc;
extern crate gstreamer_rtsp_server as gst_rtsp_server;
extern crate azure_iot_rs_sys as azure;

mod plugins;

use gst::prelude::*;
use gst_rtsp_server::prelude::*;
use gstreamer_app::{AppSrc, AppSink};
use gst::format::{GenericFormattedValue};
use gst::{Format, Bin, Element};
use gstreamer_base::prelude::{BaseSrcExt};
use gst::gst_element_error;

use azure::iothub::{IotHubMessage, IotHubModuleClient, IotHubModuleEvent};

use serde_json::{Value};

use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::str;
use std::result::Result;
use serde_derive::{Deserialize, Serialize};

use tokio::task;
use futures::join;
use futures::{StreamExt, SinkExt};

use anyhow::{Error};
use warp::Filter;
use warp::ws::Message;


fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    plugins::register(plugin)?;
    Ok(())
}

#[derive(Clone)]
struct Client {
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


struct Context {
    pipeline: Option<Element>,
    videosink: Option<Element>,
    audiosink: Option<Element>,
    webrtc_pipeline: Option<String>,
}

lazy_static! {
    static ref CONTEXT: Arc<Mutex<Context>> = {
        Arc::new(Mutex::new(Context {
        pipeline: None,
        videosink: None,
        audiosink: None,
        webrtc_pipeline: None
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

fn start_webrtc_pipeline(sender: SyncSender<Message>) -> anyhow::Result<Client> {

    let guard = CONTEXT.lock().unwrap();
    if let None = (*guard).webrtc_pipeline {
        return Err(anyhow!("No pipeline found!"));
    }

    let pipeline = (*guard).webrtc_pipeline.as_ref().unwrap();
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
 
    let webrtcbin = bin.get_by_name_recurse_up("webrtcbin").unwrap();
    let client = Client {
        sender: sender,
        webrtcbin: webrtcbin.clone() 
    };

    let client2 = client.clone();
    webrtcbin.connect("on-negotiation-needed", false, move |values| {
        let _webrtc = values[0].get::<gst::Element>().unwrap();
        if let Err(err) = on_negotiation_needed(client2.clone()) {
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
        if let Err(err) = on_ice_candidate(client3.clone(), mlineindex, candidate) {
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

fn on_offer_created(client: Client, reply: Result<Option<&gst::StructureRef>, gst::PromiseError>)-> anyhow::Result<()> {

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
    client.webrtcbin
        .emit("set-local-description", &[&offer, &None::<gst::Promise>])
        .unwrap();

    println!(
        "sending SDP offer to peer: {}",
        offer.get_sdp().as_text().unwrap()
    );

    let message = serde_json::to_string(&JsonMsg::Sdp {
        type_: "offer".to_string(),
        sdp: offer.get_sdp().as_text().unwrap(),
    })
    .unwrap();

    send_text_message(&client, &message);

    Ok(())
}

fn on_negotiation_needed(client: Client) -> Result<(), failure::Error> {
    info!("Creating negotiation offer");

    let clone = client.clone();
    let promise = gst::promise::Promise::with_change_func(move |reply| {

        let client = clone.clone();
        if let Err(err) = on_offer_created(client, reply) {
            gst_element_error!(
                clone.webrtcbin,
                gst::LibraryError::Failed,
                ("Failed to send SDP offer: {:?}", err)
            );
        }
    });

    client.webrtcbin
        .emit("create-offer", &[&None::<gst::Structure>, &promise])
        .unwrap();

    Ok(())
}

fn on_ice_candidate(client: Client, mlineindex: u32, candidate: String) -> Result<(), failure::Error> {
    let message = serde_json::to_string(&JsonMsg::Ice {
        candidate,
        sdp_mline_index: mlineindex,
    })
    .unwrap();

    send_text_message(&client, &message);
    Ok(())
}

fn send_text_message(client: &Client, message: &str) {
    let text_message = Message::text(message);
    client.sender.send(text_message).unwrap();
}

fn send_message(client: Client , message: Message) {
    client.sender.send(message).unwrap();
}

fn process_message(client: Client, message: Message) -> anyhow::Result<()> {
    let msg = message.to_str().unwrap();
    if msg.starts_with("ERROR") {
        bail!("Got error message: {}", msg);
    }

    let json_msg: JsonMsg = serde_json::from_str(msg)?;

    match json_msg {
        JsonMsg::Sdp { type_, sdp } => handle_sdp(client, &type_, &sdp),
        JsonMsg::Ice {
            sdp_mline_index,
            candidate,
        } => handle_ice(client, sdp_mline_index, &candidate),
    }

}

fn handle_ice(client: Client, sdp_mline_index: u32, candidate: &str) -> Result<(), anyhow::Error> {
    client.webrtcbin
        .emit("add-ice-candidate", &[&sdp_mline_index, &candidate])
        .unwrap();

    Ok(())
}


fn on_answer_created(client: Client,  reply: Result<Option<&gst::StructureRef>, gst::PromiseError>) -> Result<(), anyhow::Error> {
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
    client.webrtcbin
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

    send_text_message(&client, &message);

    Ok(())
}

fn handle_sdp(client: Client, type_: &str, sdp: &str) -> Result<(), Error> {
    if type_ == "answer" {
        info!("Received answer: {}\n", sdp);

        let ret = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
            .map_err(|_| anyhow!("Failed to parse SDP answer"))?;
        let answer =
            gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Answer, ret);

        client.webrtcbin
            .emit("set-remote-description", &[&answer, &None::<gst::Promise>])
            .unwrap();

        Ok(())
    } else if type_ == "offer" {
        print!("Received offer:\n{}\n", sdp);

        let ret = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
            .map_err(|_| anyhow!("Failed to parse SDP offer"))?;

        // And then asynchronously start our pipeline and do the next steps. The
        // pipeline needs to be started before we can create an answer
        let clone = client.clone();
        client.webrtcbin.call_async(move |_pipeline| {

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

                if let Err(err) = on_answer_created(app_clone.clone(), reply) {
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
    let webrtc_pipeline = &settings["desired"]["webrtc_pipeline"];
    if let Value::String(desired) = desired {
        create_pipeline(desired);
        if let Value::String(webrtc_pipeline) = webrtc_pipeline {
            let mut guard = CONTEXT.lock().unwrap();
            (*guard).webrtc_pipeline = Some(webrtc_pipeline.to_string());
        }

        task::spawn_blocking(move || { process_pipeline(); });
        if let Value::String(rtsp_pipeline) = rtsp_pipeline {
            let pipeline = rtsp_pipeline.to_string();
            task::spawn_blocking(move || { rtsp_server(pipeline); });
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

    let mut guard = CONTEXT.lock().unwrap();
    let bin = pipeline.as_ref().unwrap().downcast_ref::<Bin>().unwrap();
    (*guard).videosink = bin.get_by_name_recurse_up("video");
    (*guard).audiosink = bin.get_by_name_recurse_up("audio");
 
    pipeline.as_ref().unwrap()
        .set_state(gst::State::Playing)
        .expect("Unable to set the pipeline to the `Playing` state");

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

fn on_need_data_time(source: &AppSrc, audio: bool ) {
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
                info!("Found buffer {}", pts);
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
            let buffer_ref = copy.make_mut();
            buffer_ref.set_pts(pts);
            buffer_ref.set_dts(dts);
            let result  = source.push_buffer(copy);
            match result {
                Ok(_) => { info!("successfully pushed buffer {} {}", pts, dts); },
                Err(err) => { error!("Failed to push buffer {} {} {}", err, pts, dts)}
            }
        }
    }
}

fn on_need_data(source: &AppSrc, audio: bool) {

    let guard = CONTEXT.lock().unwrap();
    let sink =  if audio {
        (*guard).audiosink.as_ref().unwrap()
    } else {
        (*guard).videosink.as_ref().unwrap()
    };

    let appsink = sink.downcast_ref::<AppSink>().unwrap();
    let sample = appsink.pull_sample();

    if let Ok(sample) = sample {
        let buffer = sample.get_buffer_owned();
        if let Some(buffer) = buffer {
            let pts = buffer.get_pts();
            let dts = buffer.get_dts();
            let result  = source.push_buffer(buffer);
            match result {
                Ok(_) => { info!("successfully pushed buffer {} {}", pts, dts); },
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

fn rtsp_server(pipeline: String) {
    let main_loop = glib::MainLoop::new(None, false);
    let server = gst_rtsp_server::RTSPServer::new();
    let mounts = server.get_mount_points().unwrap();
    let factory = gst_rtsp_server::RTSPMediaFactory::new();
    factory.set_launch(&pipeline);
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

async fn handle_webscoket(socket: warp::ws::WebSocket) {

    let (mut writer, mut reader) = socket.split();
    let (sender, receiver) = sync_channel::<Message>(1);

    let send = task::spawn(async move {
        let webrtc_client = start_webrtc_pipeline(sender).unwrap();
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
                process_message(webrtc_client.clone(), msg).unwrap();
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

async fn http_server() {

    info!("Initializing HTTP server...");

    let ws = warp::path("ws")
    .and(warp::ws())
    .map(|ws: warp::ws::Ws| {
        ws.on_upgrade(handle_webscoket)
    });

    let root = warp::path("wwwroot")
        .and(warp::fs::dir("wwwroot"));    

    let media_path = std::env::var("media_root").unwrap_or(String::from("/tmp"));
    let media = warp::path("media")
    .and(warp::fs::dir(media_path));    

    let routes = warp::get().and(media.or(ws).or(root));

    info!("started http server....");
    warp::serve(routes)
    .run(([0, 0, 0, 0], 8000)).await;
}

#[tokio::main]
async fn main() {

    env_logger::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gst::init().unwrap();
    plugin_register_static().unwrap();
    info!("Initialized gstreamer");

    let server = http_server();
    let iot = task::spawn_blocking(move || { handle_iot(); });
    let _ = join!(server, iot);
}