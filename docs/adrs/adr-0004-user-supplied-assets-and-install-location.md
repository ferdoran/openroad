# ADR 0004: User-Supplied Assets and Install Location

Date: 2026-02-09
Status: Accepted

## Context
The project is open source and should not redistribute proprietary assets. The client is intended to operate using an existing SRO installation that the user provides.

## Decision
Do not ship or link SRO client assets. The client is designed to work when placed inside the Silkroad folder, using user-provided PK2 files and configuration.

## Consequences
Users are responsible for obtaining a compatible SRO client. The repository remains clear of proprietary assets, and deployment requires local configuration and PK2 files.
