extern crate gstreamer_rtsp_server as gst_rtsp_server;

use gst::prelude::*;
use gst_rtsp_server::prelude::*;
use gstreamer_app::{AppSrc, AppSink};
use gst::format::{GenericFormattedValue};
use gst::{Format, Bin, Element, Sample};
use gstreamer_base::prelude::{BaseSrcExt};

#[derive(Clone, Debug)]
pub struct RtspContext {
    pub audiosrc: Option<Element>,
    pub videosrc: Option<Element>,
    pub audiosink: Option<Element>,
    pub videosink: Option<Element>
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
                Ok(_) => { info!("successfully pushed buffer {}/{} ==> {}/{}", odts, opts, dts, pts); },
                Err(err) => { error!("Failed to push buffer {:?} {} {}", err, pts, dts); }
            }
        }
    }
    
    fn push_buffer(source: &AppSrc, sample: Sample) -> bool {
        let buffer = sample.get_buffer_owned();
        if let Some(buffer) = buffer {
            let pts = buffer.get_pts();
            let dts = buffer.get_dts();
            let result  = source.push_buffer(buffer);
            match result {
                Ok(status) => { 
                    info!("successfully pushed  {:?} {} {}",status, dts, pts);
                    return true; 
                },
                Err(err) => { 
                    error!("Failed to push buffer {:?} {} {}", err, pts, dts); 
                }
            };
        }
        false
    }
    
    fn on_need_data(&self, source: &AppSrc, audio: bool) {
        let sink =  if audio { 
            self.audiosink.as_ref().unwrap()
        } else {
            self.videosink.as_ref().unwrap()
        };
    
        let appsink = sink.downcast_ref::<AppSink>().unwrap();
        loop {
            let sample = appsink.pull_sample();
            if let Ok(sample) = sample {
                let result = RtspContext::push_buffer(source, sample);
                if !result {
                    break;
                }
            } else {
                break;
            }       
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

        if let Some(ref video) = context.videosrc {
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
    
    pub fn rtsp_server(&self, pipeline: &str) {
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

