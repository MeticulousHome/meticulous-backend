#!/bin/sh
set -eu

IMAGE_CHANNEL="${IMAGE_CHANNEL:-factory}"
echo ${IMAGE_CHANNEL} > /tmp/met_image-build-channel
docker compose up --remove-orphans --build --wait
