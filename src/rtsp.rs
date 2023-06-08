use crate::media::AppSinks;
use gstreamer::{
    prelude::{Cast, ObjectExt},
    traits::{ElementExt, GstBinExt},
    Bin, Clock, Element,
};
use gstreamer_rtsp_server::{
    prelude::RTSPServerExtManual,
    traits::{RTSPMediaExt, RTSPMediaFactoryExt, RTSPMountPointsExt, RTSPServerExt},
    RTSPMedia, RTSPMediaFactory, RTSPServer,
};
use log::info;

#[derive(Clone, Debug)]
pub struct RtspContext {
    pub audiosrc: Option<Element>,
    pub videosrc: Option<Element>,
    pub audiosink: Option<Element>,
    pub videosink: Option<Element>,
    pub clock: Option<Clock>,
}

impl RtspContext {
    pub fn new(sinks: AppSinks) -> Self {
        RtspContext {
            audiosink: sinks.audiosink,
            videosink: sinks.videosink,
            audiosrc: None,
            videosrc: None,
            clock: None,
        }
    }

    fn connect_sink_source(&self, sink: &Element, source: &Element) {
        source.set_property("proxysink", sink);
        source.set_base_time(sink.base_time().unwrap());
    }

    fn on_media_configure(&self, _: &RTSPMediaFactory, media: &RTSPMedia) {
        let mut context = self.clone();
        let element = media.element();
        media.set_clock(self.clock.as_ref());

        let bin = element.downcast::<Bin>().unwrap();
        context.videosrc = bin.by_name_recurse_up("videosrc");
        context.audiosrc = bin.by_name_recurse_up("audiosrc");

        if let Some(ref videosrc) = context.videosrc {
            if let Some(ref videosink) = context.videosink {
                context.connect_sink_source(videosink, videosrc);
            }
        }

        if let Some(ref audiosrc) = context.audiosrc {
            if let Some(ref audiosink) = context.audiosink {
                context.connect_sink_source(audiosink, audiosrc);
            }
        }
    }

    pub fn start(&self, pipeline: &str) {
        let rtsp_pipeline = shellexpand::env(pipeline).unwrap();
        info!("expanded RTSP pipeline: {}", rtsp_pipeline);

        let server = RTSPServer::default();
        let mounts = server.mount_points().unwrap();
        let factory = RTSPMediaFactory::default();
        factory.set_launch(&rtsp_pipeline);
        factory.set_shared(true);
        let context = self.clone();
        factory.connect_media_configure(move |factory, media| {
            context.on_media_configure(factory, media);
        });
        factory.connect_media_constructed(|_, _| {
            info!("successfully constructed RTSP media !!");
        });
        // Now we add a new mount-point and tell the RTSP server to serve the content
        // provided by the factory we configured above, when a client connects to
        // this specific path.
        mounts.add_factory("/player", factory);

        let _id = server.attach(None);

        let address = server.address().unwrap();
        info!(
            "Stream ready at rtsp://{}:{}/player",
            address.as_str(),
            server.bound_port()
        );
    }
}
