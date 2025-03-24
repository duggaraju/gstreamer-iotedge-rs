#[macro_use]
extern crate log;
extern crate anyhow;
extern crate gstreamer_rtsp_server;
extern crate std;

use std::path::Path;

use gstreamer::glib::subclass::prelude::*;
use gstreamer::glib::{self, Object, Value};
use gstreamer::{
    Bin, ClockTime, Element, ElementFactory, Message, MessageRef, MessageView, Pipeline, Registry,
    SeekFlags, SeekType, ELEMENT_METADATA_KLASS,
};
use gstreamer_rtsp_server::prelude::*;
use gstreamer_rtsp_server::subclass::prelude::*;

#[derive(Default, Debug)]
#[allow(dead_code)]
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
    type ParentType = gstreamer::Bin;

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
    fn handle_message(&self, message: Message) {
        let view = message.view();
        info!("Received message by bin: {:?}", view);
        if let MessageView::SegmentDone(_d) = view {
            warn!("Bin Segment Done bye!!");
        } else if let MessageView::StreamCollection(streams) = view {
            info!("Received stream collection {:?}", streams);
        }
        self.parent_handle_message(message)
    }
}

glib::wrapper! {
    pub struct ReplayBin(ObjectSubclass<ReplayBinImpl>) @extends Element, Bin;
}

unsafe impl Send for ReplayBin {}
unsafe impl Sync for ReplayBin {}
impl Default for ReplayBin {
    // Creates a new instance of our factory
    fn default() -> Self {
        Object::new::<Self>()
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
    fn handle_message(&self, message: &MessageRef) -> bool {
        //info!("Media received message {:?}", message.view());
        if let MessageView::SegmentDone(d) = message.view() {
            warn!("Media EOS. segment done. !! {:?}", d);
            let bin = self.obj().element();
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
        self.parent_handle_message(message)
    }

    fn prepared(&self) {
        info!("Media is prepared do a seek");
        let bin = self.obj().element();
        bin.seek(
            1.0,
            SeekFlags::SEGMENT | SeekFlags::ACCURATE | SeekFlags::FLUSH,
            SeekType::Set,
            ClockTime::ZERO,
            SeekType::None,
            ClockTime::ZERO,
        )
        .unwrap();
        self.parent_prepared();
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
    path: String,
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
        let _payloaders = Registry::get().features_filtered(
            |feature| {
                let factory = feature.downcast_ref::<ElementFactory>();
                if let Some(factory) = factory {
                    let klass = factory.metadata(ELEMENT_METADATA_KLASS).unwrap();
                    return klass.contains("Payloader") && klass.contains("RTP");
                }
                false
            },
            false,
        );
        info!("Found {} payloders", 0);
        let media_root = std::env::var("media_root").unwrap_or(String::from("/media/"));

        Self { path: media_root }
    }
}

// Implementation of glib::Object virtual methods
impl ObjectImpl for FactoryImpl {
    fn constructed(&self) {
        self.parent_constructed();
        self.obj().set_media_gtype(Media::static_type());
    }
}

// Implementation of gst_rtsp_server::RTSPMediaFactory virtual methods
impl RTSPMediaFactoryImpl for FactoryImpl {
    fn create_element(&self, url: &gstreamer_rtsp::RTSPUrl) -> Option<Element> {
        let uri = url.request_uri();
        let components = url.decode_path_components();
        info!(
            "Creating media for Base path: {} , URL {}, components: {:?}",
            self.path, uri, components
        );
        let file = Path::new(&self.path).join(components[2].as_str());
        info!("Mapped URL {} to path {:?}", uri, file);

        if !file.exists() {
            return None;
        }

        //let bin = Bin::new(None);
        let bin = ReplayBin::default();

        let src = ElementFactory::make("filesrc")
            .property("location", file.to_str())
            .build()
            .unwrap();

        let parser = ElementFactory::make("parsebin")
            .name("pb")
            .property("expose-all-streams", false)
            .build()
            .unwrap();

        let pt: u32 = 96;
        let pay = ElementFactory::make("rtph264pay")
            .name("pay0")
            .property("pt", pt)
            .build()
            .unwrap();

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

        parser.connect("autoplug-query", false, |args| -> Option<Value> {
            info!(
                "Auto plug query called!! {:?} {:?} {:?}",
                args[0], args[1], args[2]
            );

            Some(false.to_value())
        });

        bin.add_many(&[&src, &parser, &pay]).unwrap();
        Element::link_many(&[&src, &parser]).unwrap();

        Some(bin.upcast())
    }

    fn create_pipeline(&self, media: &gstreamer_rtsp_server::RTSPMedia) -> Option<Pipeline> {
        let pipeline = self.parent_create_pipeline(media);
        if pipeline.is_some() {
            let _pipeline = pipeline.as_ref().unwrap().clone();
            let bus = _pipeline.bus().unwrap();
            _ = bus
                .add_watch(move |_, mesg| {
                    info!("Received message {:?}", mesg);
                    glib::ControlFlow::Continue
                })
                .expect("msg bus error");
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
        glib::Object::new()
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
    fn make_path(&self, url: &gstreamer_rtsp::RTSPUrl) -> Option<glib::GString> {
        let path = url.decode_path_components();
        let uri = url.request_uri();
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
        glib::Object::new()
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gstreamer::init()?;
    let main_loop = glib::MainLoop::new(None, false);
    info!("Initialized gstreamer");

    let server = gstreamer_rtsp_server::RTSPServer::new();
    let mounts = MountPoints::default();
    server.set_service("8555");
    server.set_mount_points(Some(&mounts));

    let factory = Factory::default();
    factory.set_shared(true);

    mounts.add_factory("/media", factory);
    let id = server.attach(None)?;

    info!(
        "Started RTSP server. Make a request for rtsp://0.0.0.0:{}/media/filename.mp4",
        server.bound_port()
    );
    main_loop.run();
    id.remove();
    Ok(())
}
