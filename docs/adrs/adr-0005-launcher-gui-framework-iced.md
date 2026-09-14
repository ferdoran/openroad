# ADR 0005: Launcher GUI Framework (Iced)

Date: 2026-02-09
Status: Accepted

## Context
The project needs a cross-platform GUI launcher with a highly customized, image-driven skin (PK2 assets). The UI must not rely on HTML/WebView rendering. Licensing should remain permissive for potential proprietary distribution, which rules out GPLv3 dependencies.

## Decision
Use `iced` (MIT) as the Rust GUI framework for the launcher UI.

## Consequences
We can ship a native, cross-platform UI without a web runtime and keep permissive licensing. Building a pixel-exact skin will require more manual layout and styling in Rust than a declarative DSL, but the tradeoff avoids GPLv3 copyleft and keeps distribution simple. If licensing constraints change, re-evaluating a DSL-centric framework (e.g., Slint) may reduce UI implementation effort.
