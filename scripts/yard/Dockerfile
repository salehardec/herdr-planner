# The shipyard image of herdr-planner: Rust 1.99, Node 22 and Chrome for Testing, one image for the builds and the
# checks of every task of this board (PLN-48, PLN-49). The tag is the hash of this file (scripts/yard.sh), so a change
# here is a new image; the old one is left for `scripts/yard.sh prune`. The docker build context is this file alone
# (`docker build - < scripts/yard/Dockerfile`): nothing is copied in, the sources arrive by rsync through the wrapper.
#
# The versions are pinned on purpose: the base image by tag, Node by its major (NodeSource), Chrome by an exact build.
# The browser may differ from the one CI uses; CI stays the gate, the shipyard is for the work before it.
FROM rust:1.99-bookworm

# The planner shows this image and the cache volume apart from the task containers: it never offers them for cleaning.
LABEL herdr.shared=herdr-planner

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/cache/cargo \
    npm_config_cache=/cache/npm \
    CHROMIUM=/opt/chrome/chrome \
    LANG=C.UTF-8

# The page's browser libraries and the tools the checks call: rsync (the sources come through `docker exec`), ss
# (a server on our port is ours), timeout, flock, jq (the installer check), sqlite3 (the shipyard's journal check), curl.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg jq rsync iproute2 procps util-linux sqlite3 \
        libnss3 libgbm1 libasound2 libatk-bridge2.0-0 libgtk-3-0 libxkbcommon0 \
        libxrandr2 libxcomposite1 libxdamage1 libxfixes3 libdrm2 libcups2 libdbus-1-3 \
        libx11-xcb1 libpango-1.0-0 libpangocairo-1.0-0 libcairo2 \
        libexpat1 libuuid1 \
        fonts-liberation fonts-dejavu-core \
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Chrome for Testing, the exact build the checks were seen green on; `@puppeteer/browsers` puts it beside the tools.
RUN set -eux; \
    npx --yes @puppeteer/browsers install chrome@156.0.8078.47 --path /opt/chrome-dl > /tmp/chrome.txt; \
    path=$(tail -n 1 /tmp/chrome.txt | awk '{print $2}'); \
    test -x "$path"; \
    mkdir -p /opt/chrome; \
    ln -s "$path" /opt/chrome/chrome; \
    rm -f /tmp/chrome.txt

# Where a task works: the sources copied under the lock, the incoming rsync tree, the page and npm caches, the target of
# the build and what the checks leave behind (screenshots, the parts' text, the progress file). The cache volume gets
# these folders (and their owner) when docker fills an empty volume from the image.
RUN useradd --uid 1000 --create-home --shell /bin/bash yard \
    && mkdir -p /work/src /work/in /work/out /work/cache /work/target /cache/cargo /cache/npm /cache/slots \
    && chown -R yard:yard /work /cache

# Not root: the checks run as a plain user, as on a developer's machine and in CI. The installer check depends on it too —
# install.sh asks sudo for packages only when it is not root, and the check expects that sudo call.
USER yard

WORKDIR /work/src
