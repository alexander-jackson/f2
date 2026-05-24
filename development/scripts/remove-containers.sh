#!/usr/bin/env bash

docker container ls -a | awk '{ print $1 }' | tail -n +2 | xargs docker container rm --force
