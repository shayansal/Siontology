# `sion` in a small image, running as a user of its own, its home a volume.
#
#   docker build -t sion .
#   docker run --rm -v sion-home:/home/sion/.sion sion doctor
#
# With the browser and the desktop the agents can drive:
#
#   docker build --build-arg EXTRAS="chromium xvfb xdotool imagemagick" -t sion:desktop .
#
# Bubblewrap is in the image, but it makes sandboxes only where the
# container may create user namespaces (`docker run --security-opt
# seccomp=unconfined`, or a runtime that allows them); `sion doctor` says
# whether it can.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p sion-cli --bin sion

FROM debian:bookworm-slim
ARG EXTRAS=""
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates bubblewrap $EXTRAS \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/sion /usr/local/bin/sion
RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin sion
USER sion
WORKDIR /home/sion
ENV SION_HOME=/home/sion/.sion
VOLUME ["/home/sion/.sion"]
ENTRYPOINT ["sion"]
CMD ["doctor"]
