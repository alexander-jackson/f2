default: docker-build docker-build-servers clean run

check:
	cargo check

lint:
	cargo clippy -- -D warnings

test:
	cargo test

integration-test:
	cargo run --manifest-path ./integration-tests/Cargo.toml

validate: check lint test

docker-build:
	docker build --tag f2:debug --file ./development/Dockerfile.debug .

docker-build-servers:
	just development/servers/echo/build

clean:
	./development/scripts/remove-containers.sh

run:
	./development/scripts/run-in-docker.sh

reconcile:
	curl -v -X PUT -H "Host: localhost:3000" http://localhost:3000/reconcile

roll:
	sd 'single' 'double' development/config.yaml
	just reconcile
