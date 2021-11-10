export GST_DEBUG="3,*rtspmedia*:3,*CAPS*:3"
export pipeline="rtspsrc protocols=tcp location=${RTSP_SOURCE} name=src src. ! rtph264depay ! h264parse config-interval=-1 ! proxysink name=video src. ! rtpmp4gdepay ! aacparse ! proxysink name=audio"
export rtsp_pipeline="( proxysrc name=videosrc ! rtph264pay name=pay0 pt=96 proxysrc name=audiosrc ! rtpmp4apay name=pay1 pt=97 )"
export webrtc_pipeline="webrtcbin name=webrtcbin stun-server=stun://stun.l.google.com:19302 rtspsrc protocols=tcp location=${RTSP_SOURCE} ! queue max-size-buffers=1 ! rtph264depay ! h264parse ! rtph264pay config-interval=-1 name=payloader ! application/x-rtp,media=video,encoding-name=H264,payload=96 ! webrtcbin."
cargo run --bin=gstreamer-iotedge-rs

