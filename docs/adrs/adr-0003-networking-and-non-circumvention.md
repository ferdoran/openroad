# ADR 0003: Networking Based on Public Protocol Knowledge

Date: 2026-02-09
Status: Accepted

## Context
The client must communicate with servers using the Silkroad protocol. There are public materials that describe packet structures and handshake behavior. The project does not aim to bypass protections used in active server operations.

## Decision
Implement networking using existing public protocol knowledge and community solutions, aligned with the v1.188 baseline. The design focuses on compatibility and correctness, not on evasion or circumvention.

## Consequences
Networking behavior must stay within known protocol boundaries. Changes to server protections or protocol variations may require updates, but the project avoids techniques intended to bypass active defenses.
