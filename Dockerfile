# Builder Image

FROM ghcr.io/rust-lang/rust:nightly-bookworm

ARG DEFAULT_TARGET=x86_64-unknown-linux-gnu
ARG WINDOWS_TARGET=x86_64-pc-windows-gnu
ARG MAC_TARGET=aarch64-apple-darwin

RUN apt update -yqq
RUN apt upgrade -yqq
RUN apt install -yqq sshpass
RUN apt-get install -yqq clang pkg-config libx11-dev libasound2-dev libudev-dev lld
RUN apt-get install -yqq gcc-mingw-w64
RUN rustup default nightly
RUN rustup component add clippy --toolchain=nightly
RUN rustup target add $DEFAULT_TARGET $WINDOWS_TARGET $MAC_TARGET