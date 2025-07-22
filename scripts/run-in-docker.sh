#!/bin/sh

# Ensure the `internal` network exists, create it if it doesn't
docker network inspect internal >/dev/null 2>&1 || docker network create --driver bridge internal

docker run -it \
	-p 3000:3000 \
	-v ./f2.yaml:/tmp/config.yaml \
	-v ./crypto:/tmp/crypto \
	-v ./forkup.yaml:/app/forkup.yaml \
	-v /tmp:/tmp \
	-v /var/run/docker.sock:/var/run/docker.sock \
	--network internal \
	--env-file .env \
	f2:debug -- --config /tmp/config.yaml
