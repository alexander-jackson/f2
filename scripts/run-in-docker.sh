#!/bin/sh

# Ensure the `mesh` network exists, create it if it doesn't
docker network inspect mesh >/dev/null 2>&1 || docker network create --driver bridge mesh

docker run -it \
	-p 3000:3000 \
	-v ./f2.yaml:/tmp/config.yaml \
	-v ./crypto:/tmp/crypto \
	-v ./forkup.yaml:/app/forkup.yaml \
	-v /tmp:/tmp \
	-v /var/run/docker.sock:/var/run/docker.sock \
	--network mesh \
	--env-file .env \
	f2:debug -- --config /tmp/config.yaml

docker run -it \
	-p 3000:3000 \
	-v ./f2.yaml:/tmp/config.yaml \
	-v ./forkup.yaml:/app/forkup.yaml \
	-v /tmp:/tmp \
	-v /var/run/docker.sock:/var/run/docker.sock \
	--env-file .env \
	f2:debug -- --config /tmp/config.yaml
