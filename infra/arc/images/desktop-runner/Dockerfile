# Custom ARC runner image for Tauri desktop builds.
# Based on Ubuntu 22.04 with the GitHub Actions runner agent + all Tauri
# system dependencies pre-installed.
#
# Build:
#   docker build -t 192.168.1.100:6145/arc-runner-desktop:22.04 .
#   docker push 192.168.1.100:6145/arc-runner-desktop:22.04

FROM ubuntu:22.04

ARG RUNNER_VERSION=2.322.0
ARG RUNNER_ARCH=x64

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    # Runner agent dependencies
    ca-certificates \
    curl \
    wget \
    git \
    jq \
    sudo \
    lsb-release \
    gnupg \
    # Tauri / WebKitGTK dependencies
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libglib2.0-dev \
    libsoup-3.0-dev \
    libjavascriptcoregtk-4.1-dev \
    # Tauri build tools
    libayatana-appindicator3-dev \
    librsvg2-dev \
    patchelf \
    # General build essentials
    build-essential \
    pkg-config \
    libssl-dev \
    file \
    # Brotli for Trunk post_build hook
    brotli \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -d /home/runner -s /bin/bash runner \
    && echo "runner ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

WORKDIR /home/runner

RUN curl -fsSL "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-${RUNNER_ARCH}-${RUNNER_VERSION}.tar.gz" \
    | tar -xz \
    && ./bin/installdependencies.sh \
    && rm -rf /var/lib/apt/lists/*

RUN chown -R runner:runner /home/runner

USER runner
