FROM ubuntu
ENV DEBIAN_FRONTEND=noninteractive 
RUN apt-get update && apt-get install -y libcurl4 libssl1.1 libuuid1 \
    libgstreamer1.0-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad libgstrtspserver-1.0
COPY target/release/gstreamer-iotedge-rs /usr/local/bin/
ENTRYPOINT [ "gstreamer-iotedge-rs" ]