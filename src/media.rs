
use crate::webrtc::WebServer;
use crate::rtsp::RtspContext;
use crate::Settings;
use gst::prelude::*;
use gst::{Bin, Element};

use tokio::task;

#[derive(Clone)]
pub struct MediaPipeline {
    settings: Option<Settings>,
    pipeline: Option<Element>,
}

#[derive(Clone,Debug)]
pub struct AppSinks {
    pub audiosink: Option<Element>,
    pub videosink: Option<Element>,
}

impl MediaPipeline {
    pub fn new() -> Self {
        MediaPipeline {
            settings: None,
            pipeline: None,
        }        
    }

    pub fn get_appsinks(&self) -> AppSinks {
        if let Some(ref pipeline) = self.pipeline {
            let bin = pipeline.downcast_ref::<Bin>().unwrap();
            return AppSinks {
                videosink: bin.get_by_name_recurse_up("video"),
                audiosink: bin.get_by_name_recurse_up("audio")    
            };
        }
        AppSinks {
            videosink: None,
            audiosink: None,
        }
    }

    pub fn close(&mut self) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_state(gst::State::Null).expect("failed to set pipeline to null");
            self.pipeline = None;
        }
    }

    fn start(&mut self, pipeline: &str) {
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
        self.pipeline = pipeline.ok();
    
        self.process();
    } 
    
    fn process(&self) {
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

    pub fn update(&mut self, settings: Settings) {
        if let Some(ref pipeline) = settings.pipeline {
            self.start(pipeline);
        }

        if let Some(ref pipeline) = settings.rtsp_pipeline {

            let rtsp_context = RtspContext::new(self.get_appsinks());
            rtsp_context.start(pipeline);
        }

        let http = settings.http_server;
        let pipeline = settings.webrtc_pipeline.as_ref().map_or(String::new(), String::from);
        self.settings = Some(settings);
        if http {
            task::spawn( async move { 
                let _server = WebServer::start(pipeline).await;
            });
        }    
    }
}
