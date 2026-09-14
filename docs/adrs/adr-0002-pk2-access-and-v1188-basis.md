# ADR 0002: PK2 Access Based on v1.188 Materials

Date: 2026-02-09
Status: Accepted

## Context
We must load SRO assets from PK2 archives without actively performing reverse engineering ourselves. Publicly available materials for v1.188 provide enough structure to interpret PK2 content and related formats.

## Decision
Implement a custom PK2 AssetReader and build initial format support on the v1.188 knowledge base. This version is treated as the first compatibility plugin, with future versions to be added as separate modules.

## Consequences
The client requires user-supplied PK2 files and will initially be compatible with v1.188 data layouts. Additional versions can be added over time without rewriting the core asset pipeline.
