#!/bin/sh
export IMAGE_CHANNEL="factory"
echo ${IMAGE_CHANNEL} > /tmp/met_image-build-channel
docker compose run --remove-orphans --build -p 8080:8080 -v /tmp/met_image-build-channel:/opt/image-build-channel -v $(pwd):/app backend
