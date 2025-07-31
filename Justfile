default: build clean run

check:
	cargo check

lint:
	cargo clippy -- -D warnings

test:
	cargo test

validate: check lint test

build:
	docker build -t f2:debug .

clean:
	./scripts/remove-containers.sh

run:
	./scripts/run-in-docker.sh

reconcile:
	curl -v -H "Host: localhost:3000" http://localhost:3000/reconcile

roll:
	sd 'former' 'latter' f2.yaml
	just reconcile
	sd 'latter' 'former' f2.yaml
	just reconcile
