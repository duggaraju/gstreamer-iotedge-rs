#[macro_use]
extern crate log;
extern crate anyhow;
extern crate glib;
extern crate gst;
extern crate gstreamer_rtsp_server;
extern crate std;

use std::path::Path;

use anyhow::{Error, Result};
use glib::Value;
use glib::subclass::prelude::*;
use gst::{ClockTime, Message, MessageView, SeekFlags, SeekType, ELEMENT_METADATA_KLASS};
use gstreamer_rtsp_server::prelude::*;
use gstreamer_rtsp_server::subclass::prelude::*;

#[derive(Default, Debug)]
pub struct ReplayBinImpl {
    pt: u32,
    path: String,
}

impl ReplayBinImpl {
    fn media_root() -> String {
        std::env::var("media_root").unwrap_or(String::from("/media/test.mp4"))
    }
}

#[glib::object_subclass]
impl ObjectSubclass for ReplayBinImpl {
    const NAME: &'static str = "ReplayBin";
    type Type = ReplayBin;
    type ParentType = gst::Bin;

    fn new() -> Self {
        Self {
            pt: 96,
            path: Self::media_root(),
        }
    }
}

impl ElementImpl for ReplayBinImpl {}
impl GstObjectImpl for ReplayBinImpl {}

impl ObjectImpl for ReplayBinImpl {}

impl BinImpl for ReplayBinImpl {
    fn handle_message(&self, bin: &Self::Type, message: Message) {
        let view = message.view();
        info!("Received message by bin: {:?}", view);
        if let MessageView::SegmentDone(d) = view {
            warn!("Bin Segment Done bye!!");
        } else if let MessageView::StreamCollection(streams) = view {
            info!("Received stream collection {:?}", streams);
        }
        self.parent_handle_message(bin, message)
    }
}

glib::wrapper! {
    pub struct ReplayBin(ObjectSubclass<ReplayBinImpl>) @extends gst::Element, gst::Bin;
}

unsafe impl Send for ReplayBin {}
unsafe impl Sync for ReplayBin {}
impl Default for ReplayBin {
    // Creates a new instance of our factory
    fn default() -> Self {
        glib::Object::new(&[]).expect("Failed to create factory")
    }
}

#[derive(Default)]
pub struct MediaImpl {}

// This trait registers our type with the GObject object system and
// provides the entry points for creating a new instance and setting
// up the class data
#[glib::object_subclass]
impl ObjectSubclass for MediaImpl {
    const NAME: &'static str = "RsRTSPMedia";
    type Type = Media;
    type ParentType = gstreamer_rtsp_server::RTSPMedia;
}

// Implementation of glib::Object virtual methods
impl ObjectImpl for MediaImpl {}

// Implementation of gst_rtsp_server::RTSPMedia virtual methods
impl RTSPMediaImpl for MediaImpl {
    fn handle_message(&self, media: &Self::Type, message: &gst::MessageRef) -> bool {
        //info!("Media received message {:?}", message.view());
        if let MessageView::SegmentDone(d) = message.view() {
            warn!("Media EOS. segment done. !! {:?}", d);
            let bin = media.element().unwrap();
            bin.seek(
                1.0,
                SeekFlags::SEGMENT | SeekFlags::ACCURATE,
                SeekType::Set,
                ClockTime::ZERO,
                SeekType::None,
                ClockTime::ZERO,
            )
            .unwrap();
        }
        self.parent_handle_message(media, message)
    }

    fn prepared(&self, media: &Self::Type) {
        info!("Media is prepared do a seek");
        let bin = media.element().unwrap();
        bin.seek(
            1.0,
            SeekFlags::SEGMENT | SeekFlags::ACCURATE | SeekFlags::FLUSH,
            SeekType::Set,
            ClockTime::ZERO,
            SeekType::None,
            ClockTime::ZERO,
        )
        .unwrap();
        self.parent_prepared(media);
    }
}

// This here defines the public interface of our factory and implements
// the corresponding traits so that it behaves like any other RTSPMedia
glib::wrapper! {
    pub struct Media(ObjectSubclass<MediaImpl>) @extends gstreamer_rtsp_server::RTSPMedia;
}

// Medias must be Send+Sync, and ours is
unsafe impl Send for Media {}
unsafe impl Sync for Media {}

// This is the private data of our factory
#[derive(Default, Debug)]
pub struct FactoryImpl {
    payloaders: Vec<gst::PluginFeature>,
    path: String
}

// This trait registers our type with the GObject object system and
// provides the entry points for creating a new instance and setting
// up the class data
#[glib::object_subclass]
impl ObjectSubclass for FactoryImpl {
    const NAME: &'static str = "RTSPMediaFactory";
    type Type = Factory;
    type ParentType = gstreamer_rtsp_server::RTSPMediaFactory;

    fn with_class(_klass: &Self::Class) -> Self {
        let payloaders = gst::Registry::get().features_filtered(
            |feature| {
                let factory = feature.downcast_ref::<gst::ElementFactory>();
                if let Some(factory) = factory {
                    let klass = factory.metadata(&ELEMENT_METADATA_KLASS).unwrap();
                    return klass.contains("Payloader") && klass.contains("RTP");
                }
                false
            },
            false,
        );
        info!("Found {} payloders", 0);
        let media_root = std::env::var("media_root").unwrap_or(String::from("/media/"));

        Self { payloaders: Vec::new(), path: media_root }
    }
}

// Implementation of glib::Object virtual methods
impl ObjectImpl for FactoryImpl {
    fn constructed(&self, factory: &Self::Type) {
        self.parent_constructed(&factory);
        factory.set_media_gtype(Media::static_type());
    }
}

// Implementation of gst_rtsp_server::RTSPMediaFactory virtual methods
impl RTSPMediaFactoryImpl for FactoryImpl {
    fn create_element(
        &self,
        _factory: &Self::Type,
        url: &gstreamer_rtsp::RTSPUrl,
    ) -> Option<gst::Element> {
        let uri = url.request_uri().unwrap();
        let components = url.decode_path_components();
        info!("Creating media for Base path: {} , URL {}, components: {:?}", self.path, uri, components);
        let file = Path::new(&self.path).join(components[2].as_str());
        info!("Mapped URL {} to path {:?}", uri, file);

        if !file.exists() {
            return None;
        }

        //let bin = gst::Bin::new(None);
        let bin = ReplayBin::default();

        let src = gst::ElementFactory::make("filesrc", None).unwrap();
        src.set_property("location", file.to_str());

        let parser = gst::ElementFactory::make("parsebin", Some("pb")).unwrap();
        parser.set_property("expose-all-streams", &false);

        let pay = gst::ElementFactory::make("rtph264pay", Some("pay0")).unwrap();
        let pt: u32 = 96;
        pay.set_property("pt", &pt);

        let _bin = bin.clone();
        let _pay = pay.clone();
        parser.connect_pad_added(move |demux, src_pad| {
            let caps = src_pad.current_caps().unwrap_or(src_pad.query_caps(None));
            let name = caps.structure(0).unwrap().name();
            info!(
                "Added pad {} to {} caps: {:?}",
                src_pad.name(),
                demux.name(),
                caps
            );
            if name.starts_with("video/") {
                let sink_pad = &_pay.sink_pads()[0];
                src_pad.link(sink_pad).unwrap();    
            }
        });

        parser.connect("autoplug-query", false, | args | -> Option<Value> {
            info!("Auto plug query called!! {:?} {:?} {:?}", args[0], args[1], args[2]);

            Some(false.to_value())
        });

        bin.add_many(&[&src, &parser, &pay]).unwrap();
        gst::Element::link_many(&[&src, &parser]).unwrap();

        Some(bin.upcast())
    }

    fn create_pipeline(
        &self,
        factory: &Self::Type,
        media: &gstreamer_rtsp_server::RTSPMedia,
    ) -> Option<gst::Pipeline> {
        let pipeline = self.parent_create_pipeline(factory, media);
        if pipeline.is_some() {
            let _pipeline = pipeline.as_ref().unwrap().clone();
            let bus = _pipeline.bus().unwrap();
            bus.add_watch(move |_, mesg| {
                info!("Received message {:?}", mesg);
                glib::Continue(true)
            }).unwrap();
        }
        pipeline
    }
}

// This here defines the public interface of our factory and implements
// the corresponding traits so that it behaves like any other RTSPMediaFactory
glib::wrapper! {
    pub struct Factory(ObjectSubclass<FactoryImpl>) @extends gstreamer_rtsp_server::RTSPMediaFactory;
}

// Factories must be Send+Sync, and ours is
unsafe impl Send for Factory {}
unsafe impl Sync for Factory {}
impl Default for Factory {
    // Creates a new instance of our factory
    fn default() -> Factory {
        glib::Object::new(&[]).expect("Failed to create factory")
    }
}

#[derive(Default, Debug)]
pub struct MountPointsImpl {}

// This trait registers our type with the GObject object system and
// provides the entry points for creating a new instance and setting
// up the class data
#[glib::object_subclass]
impl ObjectSubclass for MountPointsImpl {
    const NAME: &'static str = "RTSPMountPoints";
    type Type = MountPoints;
    type ParentType = gstreamer_rtsp_server::RTSPMountPoints;
}

// Implementation of glib::Object virtual methods
impl ObjectImpl for MountPointsImpl {}

// Implementation of gst_rtsp_server::RTSPMediaFactory virtual methods
impl RTSPMountPointsImpl for MountPointsImpl {
    fn make_path(
        &self,
        _mount_points: &Self::Type,
        url: &gstreamer_rtsp::RTSPUrl,
    ) -> Option<glib::GString> {
        let path = url.decode_path_components();
        let uri = url.request_uri().unwrap();
        info!("Make path called for {} {} {:?}", uri, path.len(), path);
        if path[1].as_str() == "media" && path.len() >= 2 {
            Some(glib::GString::from("/media"))
        } else {
            None
        }
    }
}

glib::wrapper! {
    pub struct MountPoints(ObjectSubclass<MountPointsImpl>) @extends gstreamer_rtsp_server::RTSPMountPoints;
}

// Factories must be Send+Sync, and ours is
unsafe impl Send for MountPoints {}
unsafe impl Sync for MountPoints {}
impl Default for MountPoints {
    // Creates a new instance of our factory
    fn default() -> MountPoints {
        glib::Object::new(&[]).expect("Failed to create factory")
    }
}

fn main() -> Result<(), Error> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gst::init()?;
    let main_loop = glib::MainLoop::new(None, false);
    info!("Initialized gstreamer");

    let server = gstreamer_rtsp_server::RTSPServer::new();
    let mounts = MountPoints::default();
    server.set_service("8555");
    server.set_mount_points(Some(&mounts));

    let factory = Factory::default();
    factory.set_shared(true);

    mounts.add_factory("/media", &factory);
    let id = server.attach(None)?;

    info!(
        "Started RTSP server. Make a request for rtsp://0.0.0.0:{}/media/filename.mp4",
        server.bound_port()
    );
    main_loop.run();
    id.remove();
    Ok(())
}
