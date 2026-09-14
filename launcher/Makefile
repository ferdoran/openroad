APP_NAME := launcher

.PHONY: run build check fmt clippy clean

run:
	cargo run -p $(APP_NAME)

build:
	cargo build -p $(APP_NAME)

check:
	cargo check -p $(APP_NAME)

fmt:
	cargo fmt

clippy:
	cargo clippy -p $(APP_NAME) -- -D warnings

clean:
	cargo clean -p $(APP_NAME)
