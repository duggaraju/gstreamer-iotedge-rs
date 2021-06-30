extern crate gstreamer_webrtc as gst_webrtc;
extern crate serde;
extern crate serde_derive;

use gst::element_error;
use gst::prelude::*;
use gst::{Bin, Element};
use std::path::Path;

use serde_derive::{Deserialize, Serialize};
use std::result::Result;
use std::str;
use std::sync::mpsc::{sync_channel, SyncSender};

use futures::{SinkExt, StreamExt};
use tokio::task;

use anyhow::Error;
use warp::ws::Message;
use warp::Filter;

#[derive(Clone)]
pub struct WebRtcContext {
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

impl WebRtcContext {
    pub fn start(pipeline: String, sender: SyncSender<Message>) -> anyhow::Result<WebRtcContext> {
        let expanded_pipeline = shellexpand::env(&pipeline).unwrap();
        info!("expanded Web RTC  pipeline: {}", expanded_pipeline);
        let mut context = gst::ParseContext::new();
        let element = gst::parse_launch_full(
            &expanded_pipeline,
            Some(&mut context),
            gst::ParseFlags::empty(),
        );
        if let Err(err) = element {
            if let Some(gst::ParseError::NoSuchElement) = err.kind::<gst::ParseError>() {
                error!("Missing element(s): {:?}", context.missing_elements());
            } else {
                error!("Failed to parse pipeline: {}", err);
            }
            return Err(anyhow!("Failed to created webrtc pipeline!"));
        }

        let element = element.unwrap();
        let bin = element
            .downcast_ref::<Bin>()
            .expect("Element must be a bin!");

        let webrtcbin = bin
            .by_name_recurse_up("webrtcbin")
            .expect("missing element by name webrtcbin");
        let client = WebRtcContext {
            sender: sender,
            webrtcbin: webrtcbin.clone(),
        };

        let client2 = client.clone();
        webrtcbin
            .connect("on-negotiation-needed", false, move |values| {
                let _webrtc = values[0].get::<gst::Element>().unwrap();
                if let Err(err) = client2.on_negotiation_needed() {
                    element_error!(
                        _webrtc,
                        gst::LibraryError::Failed,
                        ("Failed to negotiate: {:?}", err)
                    );
                }

                None
            })
            .unwrap();

        let client3 = client.clone();
        webrtcbin
            .connect("on-ice-candidate", false, move |values| {
                let _webrtc = values[0].get::<gst::Element>().expect("Invalid argument");
                let mlineindex = values[1].get::<u32>().expect("Invalid argument");
                let candidate = values[2].get::<String>().expect("Invalid argument");
                if let Err(err) = client3.on_ice_candidate(mlineindex, candidate) {
                    element_error!(
                        _webrtc,
                        gst::LibraryError::Failed,
                        ("Failed to send ICE candidate: {:?}", err)
                    );
                }
                None
            })
            .unwrap();

        element
            .set_state(gst::State::Playing)
            .expect("Unable to set the pipeline to the `Playing` state");

        Ok(client)
    }

    fn on_offer_created(
        &self,
        reply: Result<Option<&gst::StructureRef>, gst::PromiseError>,
    ) -> anyhow::Result<()> {
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                bail!("Offer creation future got no reponse: {:?}", err);
            }
        };

        let offer = reply
            .unwrap()
            .value("offer")
            .unwrap()
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .expect("Invalid argument");
        self.webrtcbin
            .emit_by_name("set-local-description", &[&offer, &None::<gst::Promise>])
            .unwrap();

        info!(
            "sending SDP offer to peer: {}",
            offer.sdp().as_text().unwrap()
        );

        let message = serde_json::to_string(&JsonMsg::Sdp {
            type_: "offer".to_string(),
            sdp: offer.sdp().as_text().unwrap(),
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
                element_error!(
                    client.webrtcbin,
                    gst::LibraryError::Failed,
                    ("Failed to send SDP offer: {:?}", err)
                );
            }
        });

        self.webrtcbin
            .emit_by_name("create-offer", &[&None::<gst::Structure>, &promise])
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

    fn send_message(&self, message: Message) {
        self.sender.send(message).unwrap();
    }

    pub fn process_message(&self, message: Message) -> anyhow::Result<()> {
        let msg = message.to_str().unwrap();
        if msg.starts_with("ERROR") {
            bail!("Got error message: {}", msg);
        }

        let json_msg: JsonMsg = serde_json::from_str(msg)?;

        match json_msg {
            JsonMsg::Sdp { type_, sdp } => self.handle_sdp(&type_, &sdp),
            JsonMsg::Ice {
                sdp_mline_index,
                candidate,
            } => self.handle_ice(sdp_mline_index, &candidate),
        }
    }

    fn handle_ice(&self, sdp_mline_index: u32, candidate: &str) -> Result<(), anyhow::Error> {
        self.webrtcbin
            .emit_by_name("add-ice-candidate", &[&sdp_mline_index, &candidate])?;
        Ok(())
    }

    fn on_answer_created(
        &self,
        reply: Result<Option<&gst::StructureRef>, gst::PromiseError>,
    ) -> Result<(), anyhow::Error> {
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                bail!("Answer creation future got no reponse: {:?}", err);
            }
        };

        let answer = reply
            .unwrap()
            .value("answer")
            .unwrap()
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .expect("Invalid argument");
        self.webrtcbin
            .emit_by_name("set-local-description", &[&answer, &None::<gst::Promise>])
            .unwrap();

        info!(
            "sending SDP answer to peer: {}",
            answer.sdp().as_text().unwrap()
        );

        let message = serde_json::to_string(&JsonMsg::Sdp {
            type_: "answer".to_string(),
            sdp: answer.sdp().as_text().unwrap(),
        })
        .unwrap();

        self.send_text_message(&message);
        Ok(())
    }

    fn handle_sdp(&self, type_: &str, sdp: &str) -> Result<(), Error> {
        if type_ == "answer" {
            info!("Received answer: {}\n", sdp);

            let ret = gstreamer_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
                .map_err(|_| anyhow!("Failed to parse SDP answer"))?;
            let answer =
                gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Answer, ret);

            self.webrtcbin
                .emit_by_name("set-remote-description", &[&answer, &None::<gst::Promise>])
                .unwrap();

            Ok(())
        } else if type_ == "offer" {
            info!("Received offer:\n{}\n", sdp);

            let ret = gstreamer_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
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
                    .emit_by_name("set-remote-description", &[&offer, &None::<gst::Promise>])
                    .unwrap();

                let app_clone = clone.clone();
                let promise = gst::Promise::with_change_func(move |reply| {
                    if let Err(err) = app_clone.on_answer_created(reply) {
                        element_error!(
                            app_clone.webrtcbin,
                            gst::LibraryError::Failed,
                            ("Failed to send SDP answer: {:?}", err)
                        );
                    }
                });

                clone
                    .webrtcbin
                    .emit_by_name("create-answer", &[&None::<gst::Structure>, &promise])
                    .unwrap();
            });

            Ok(())
        } else {
            bail!("Sdp type is not \"answer\" but \"{}\"", type_)
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebServer {
    webrtc_pipeline: String,
}

impl WebServer {
    async fn handle_webscoket(pipeline: String, socket: warp::ws::WebSocket) {
        info!("Staring a web socket session for pipeline: {}", pipeline);
        // let pipeline = String::new();
        let (mut writer, mut reader) = socket.split();
        let (sender, receiver) = sync_channel::<Message>(1);

        let send = task::spawn(async move {
            let webrtc_client = WebRtcContext::start(pipeline, sender).unwrap();
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
            }
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

    pub async fn start(webrtc_pipeline: String) {
        info!("Initializing HTTP server...");
        let root_dir = Path::new("wwwroot");
        let root = warp::path("wwwroot").and(warp::fs::dir(root_dir));

        let media_path = std::env::var("media_root").unwrap_or(String::from("/tmp"));
        let media = warp::path("media").and(warp::fs::dir(media_path));

        let use_ws = !webrtc_pipeline.is_empty();

        // If web rtc is enabled allow web sockets.
        let ws = warp::path("ws")
            .and_then(move || async move {
                if use_ws {
                    Ok(true)
                } else {
                    Err(warp::reject::not_found())
                }
            })
            .and(warp::ws())
            .map(move |_, ws: warp::ws::Ws| {
                let pipeline = webrtc_pipeline.clone();
                ws.on_upgrade(|ws| async move {
                    WebServer::handle_webscoket(pipeline, ws).await;
                })
            });

        info!("started http server....");
        let routes = warp::get().and(media.or(ws).or(root));
        warp::serve(routes).run(([0, 0, 0, 0], 8000)).await;
    }
}
