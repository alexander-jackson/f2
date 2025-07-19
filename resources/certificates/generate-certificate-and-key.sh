#!/usr/bin/env bash

# This script generates a self-signed SSL certificate and a private key for a given sample domain.

# grab the first argument as the domain name
DOMAIN="$1"

if [ -z "$DOMAIN" ]; then
  echo "Usage: $0 <domain>"
  exit 1
fi

openssl req -newkey rsa:2048 -nodes -keyout ${DOMAIN}.key -out ${DOMAIN}.csr -subj "/C=GB/ST=England/L=London/O=Example/CN=${DOMAIN}.example.com"
openssl x509 -signkey ${DOMAIN}.key -in ${DOMAIN}.csr -req -days 365 -out ${DOMAIN}.crt

rm ${DOMAIN}.csr
