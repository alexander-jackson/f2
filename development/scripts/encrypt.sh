#!/usr/bin/env bash

echo -n $1 | openssl rsautl -pubin -inkey public.key -encrypt | base64
