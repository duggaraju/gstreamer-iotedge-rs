# gstreamer-iotedge-rs
An Azure IOT Edge module to run gstreamer pipeline

Read here(https://docs.microsoft.com/en-us/azure/iot-edge/iot-edge-modules) for more information about How Azure IOT edge modules work.

The module can run any gstreamer pipeline that you pass in as part of the module twin settings.  Apart from that, it has web server so you can playback DASH/HLS streams, a websocket server so you can do web RTC playback, an RTSP server to do RTSP playback all integrated together.

# Running a pipeline.

Test your pipleine as you would normally do.
```sh
gst-launch-1.0  element1 ! element2 ! sink2
```
Once it is working fine. Update the module twin settings of your module with those settings.

```json
"properties": {
    "desired": {
      "pipeline": "element1 ! element2 ! sink2",
      "$metadata": {
      }
```
Now when you run your IOT edge module it will run the gstream pipeline as you would on the command line.

You can use environment variables in your pipeline and they would be exapnded. e.g:
```json
"properties": {
    "desired": {
      "pipeline": "rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location=rtsp://$user:$password@cameraip/axis-media/media.amp name=src src. ! queue ! rtph264depay ! h264parse config-interval=-1 ! filesink location=$media_path",
      "$metadata": {
      }
```

#Extensibility

Easy to write additional gStreamer Elements in Rust. As an example code include a dummy element called 'inference' that just passes the input without modification and you can just add it to your pipeline
rtspsrc location=rtsp://server ! rtph264depay ! *inference* ! h264parse ! mp4muxer | filesink location=a.mp4

If you have additonal plugins that you want to run.
* Mount the directory containing the plugins to the module.
* Set the environment variable GST_PLUGIN_PATH to point to the mounted directory.
* Use the elements in your pipeline and run the module as usual 

# RTSP plabyack

Just set the rts_pipeline property of your module twin.

example:
```json
"properties": {
    "desired": {
      "pipeline": "rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location=rtsp://$USER:$PASSWORD@$IP/axis-media/media.amp name=src src. ! queue ! rtph264depay ! h264parse config-interval=-1 ! appsink name=video max-buffers=30 drop=true src. ! rtpmp4gdepay ! aacparse ! appsink name=audio drop=true max-buffers=30",
      "rtsp_pipeline": "( rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location='rtsp://$USER:$PASSWORD@$IP/axis-media/media.amp' latency=200 name=src src. ! queue ! rtph264depay ! h264parse config-interval=-1 ! rtph264pay name=pay0 pt=96 src. ! rtpmp4gdepay ! aacparse ! rtpmp4apay name=pay1 pt=97 )",
      "$metadata": {}
```

## If you want to share some some of the processing between the main pipeline and the RTSP pipeline...

* Identity the streams in your pipeline that you want to stream and send them to an appsink with name audio/video. Use tee element if needed.
* Add an rtsp_pipeline with appsrc name audiosrc/videosrc. The app will then pipe the buffers from the sink to the source and use that for playback.
* The RTSP server is by default running on port 8554. So run something like ffplay rtsp://ipaddres:8554/player to play the stream.

example:
```json
"properties": {
    "desired": {
      "pipeline": "rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location=rtsp://$USER:$PASSWORD@IP.ADDRESS/axis-media/media.amp name=src src. ! queue ! rtph264depay ! h264parse config-interval=-1 ! appsink name=video max-buffers=30 drop=true src. ! rtpmp4gdepay ! aacparse ! appsink name=audio drop=true max-buffers=30",
      "rtsp_pipeline": "( appsrc name=videosrc ! h264parse ! rtph264pay config-interval=-1 name=pay0 pt=96 )",
      "$metadata": {}
```

# WebRTC playback.

* Set the webrtc_pipeline property of your module twin.
* Launch the browser and point to http://ipaddressofmodule:8000/wwwroot/webrtc.html

```json
"properties": {
    "desired": {
      "pipeline": "rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location=rtsp://$USER:$PASSWORD@IP.ADDRESS/axis-media/media.amp name=src src. ! queue ! rtph264depay ! h264parse config-interval=-1 ! appsink name=video max-buffers=30 drop=true src. ! rtpmp4gdepay ! aacparse ! appsink name=audio drop=true max-buffers=30",
      "webrtc_pipeline": "webrtcbin name=webrtcbin stun-server=stun://stun.l.google.com:19302 rtspsrc protocols=GST_RTSP_LOWER_TRANS_TCP location=rtsp://$USER:$PASSWROD@10.91.98.185/axis-media/media.amp ! rtph264depay ! h264parse ! rtph264pay config-interval=-1 name=payloader ! application/x-rtp,media=video,encoding-name=H264,payload=96 ! webrtcbin.",
      "$metadata": {}
    }
```

# Building Code
Assuming you have rust tooling installed. All you need is
```sh
caro build 
```

To build the container:
```sh
cargoo build --release
docker build -f Dockerfile.amd64 -t gstreamer-iot .
```
Now you can push the continaer to docker hub or any other container registry. And you can register the module in your Azure IOT Hub.
# What Next ....

Support for sharing pielines between multiple WebRTC clients (Right now each player has its own pipeline)
Show how  GPU can be use for decoding.
Write a plugin to analyze the vide frames using some AI Model.
Write a plugin to save the video to Amzon S3/Azure Blob storage.
Adding support for certificates for RTSP/WebRTC playback and other authentication support.
