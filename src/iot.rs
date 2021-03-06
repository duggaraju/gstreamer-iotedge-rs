
use serde_json::Value;
use crate::http_server;
use crate::RtspContext;
use crate::ModuleSettings;
use gst::prelude::*;
use gst::{Bin, Element};

use azure_iot_rs::message::{IotHubMessage};
use azure_iot_rs::module::{IotHubModuleClient, IotHubModuleEvent};

use tokio::task;

#[derive(Clone)]
pub struct ModuleContext {
    settings: Option<ModuleSettings>,
    pipeline: Option<Element>,
    videosink: Option<Element>,
    audiosink: Option<Element>,
    webrtc_pipeline: Option<String>,
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

    fn module_callback(&mut self, event: IotHubModuleEvent) {
        let _result = match event {
            IotHubModuleEvent::Message(message) => self.handle_message(message),
            IotHubModuleEvent::Twin(settings) => self.handle_twin(settings)
        };
    }    

    fn handle_twin(&mut self, settings: Value) -> Result<(), &'static str> {
        let desired = ModuleSettings::from_json(settings);
        self.update(desired);
        Ok(())
    }    

    fn handle_message(&self, message: IotHubMessage) -> Result<(), &'static str> {
        info!("Received hub message {}", message.content_type());
        return Ok(())
    }
    
    pub fn handle_iot() {
        let mut context = ModuleContext::new();
        let mut client = IotHubModuleClient::new(|event| { context.module_callback(event); });
        info!("Initialized IOT hub module...");
        client.do_work();
    }
}
