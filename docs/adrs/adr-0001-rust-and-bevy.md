# ADR 0001: Rust and Bevy as the Client Foundation

Date: 2026-02-09
Status: Accepted

## Context
We need a cross-platform, high-performance client with modern rendering, hot-reload friendly workflows, and a strong open source ecosystem. The client must run on macOS and be maintainable by contributors.

## Decision
Use Rust as the primary language and Bevy as the game engine. The client is structured as a Bevy app with plugins for assets, networking, UI, and scenes.

## Consequences
We depend on the Rust nightly toolchain and Bevy release cadence. Most systems are implemented as ECS systems and Bevy plugins, which encourages modularity but requires Bevy-specific patterns and APIs.
