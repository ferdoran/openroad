# Paranoid static-analysis container for VSRO server drops.
#
# Read-only sandbox that ingests an archive, extracts it inside the
# container's tmpfs, runs ClamAV + YARA + binwalk + PE-import-table
# scrutiny + suspicious-string heuristics, and writes a JSON verdict
# to a host-mounted output volume.
#
# The container never has network access at runtime (`--network=none`).
# ClamAV signatures are baked in at build time. YARA rule updates also
# happen at build, so a stale image means stale defenses — rebuild
# every few weeks for fresh sigs.
#
# Build:   docker build -t sro-safe-analyze docker/safe-analyze
# Run:     see scripts/safe-analyze.sh
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      clamav clamav-freshclam \
      yara \
      binwalk \
      p7zip-full unzip \
      file \
      coreutils \
      python3 python3-pip \
      jq \
 && rm -rf /var/lib/apt/lists/*

# pefile = pure-Python PE parser (imports, sections, entropy)
RUN pip3 install --break-system-packages --no-cache-dir pefile==2024.8.26

# Bake ClamAV signatures into the image so the runtime container can
# run with --network=none. freshclam writes to /var/lib/clamav.
# Allow this to fail at build time (mirror flake) — the runtime will
# still work, just without sigs; the runtime explicitly checks for
# main.cvd|main.cld presence and warns.
RUN freshclam --quiet || true

# Custom YARA ruleset baked at /opt/rules.yar.
COPY rules.yar /opt/rules.yar

# Sanity-compile the ruleset at build time so a syntax error breaks
# the image, not the first analysis run.
RUN yara /opt/rules.yar /etc/hostname 2>&1 | grep -vE 'warning|error' >/dev/null \
 || yara /opt/rules.yar /etc/hostname

COPY analyze.py /usr/local/bin/analyze.py
RUN chmod +x /usr/local/bin/analyze.py

# Non-root user. Even if a tool has an RCE bug triggered by a crafted
# input, the blast radius is limited to the container's user namespace.
RUN useradd -m -u 1000 analyst
WORKDIR /work

USER analyst

ENTRYPOINT ["/usr/local/bin/analyze.py"]
