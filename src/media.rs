use crate::rtsp::RtspContext;
use crate::webrtc::WebServer;
use crate::Settings;
use gstreamer::{
    glib::ControlFlow, prelude::*, Bin, Clock, Element, MessageView, ParseContext, ParseFlags,
    State,
};
use log::{error, info};

#[derive(Clone)]
pub struct MediaPipeline {
    settings: Option<Settings>,
    pipeline: Option<Element>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct AppSinks {
    pub audiosink: Option<Element>,
    pub videosink: Option<Element>,
    clock: Option<Clock>,
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline
                .set_state(State::Null)
                .expect("failed to set pipeline to null");
            self.pipeline = None;
        }
    }
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
                videosink: bin.by_name_recurse_up("video"),
                audiosink: bin.by_name_recurse_up("audio"),
                clock: pipeline.clock(),
            };
        }
        AppSinks {
            videosink: None,
            audiosink: None,
            clock: None,
        }
    }

    fn start(&mut self, pipeline: &str) -> anyhow::Result<()> {
        info!("expanding environemnt variables in pipeline: {pipeline}");
        let expanded_pipeline = shellexpand::env(pipeline)?;
        let mut context = ParseContext::new();
        let pipeline = gstreamer::parse::launch_full(
            &expanded_pipeline,
            Some(&mut context),
            ParseFlags::empty(),
        )?;

        pipeline.set_state(State::Playing)?;
        self.pipeline = Some(pipeline);
        info!("started media pipeline...");
        self.process()
    }

    fn process(&self) -> anyhow::Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or(anyhow::format_err!("No pipeline"))?
            .clone();
        let bus = pipeline
            .bus()
            .expect("Failed to get the bus from pipeline!");
        let _ = bus.add_watch(move |_, msg| match msg.view() {
            MessageView::Eos(..) => {
                error!("Received EOS on the pipeline!");
                pipeline
                    .set_state(State::Null)
                    .expect("Unable to set the pipeline to the `Null` state");
                ControlFlow::Break
            }
            MessageView::Error(err) => {
                error!(
                    "Error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                ControlFlow::Break
            }
            _ => ControlFlow::Continue,
        })?;
        Ok(())
    }

    pub fn update(&mut self, settings: Settings) -> anyhow::Result<()> {
        if let Some(ref pipeline) = settings.pipeline {
            self.start(pipeline)?;
        }

        if let Some(ref pipeline) = settings.rtsp_pipeline {
            let rtsp_context = RtspContext::new(self.get_appsinks());
            rtsp_context.start(pipeline)?;
        }

        let http = settings.http_server;
        let pipeline = settings.webrtc_pipeline.clone().unwrap_or_default();
        self.settings = Some(settings);
        if http {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    WebServer::start(pipeline).await;
                });
            });
        }
        Ok(())
    }
}
