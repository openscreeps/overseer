# The `community` e2e target's server (see xtask/src/e2e/community.rs): screeps-launcher with
# the packages of config.yml and the initial world installed at build time, so starting a world
# needs no network (but for the backend's Steam Web API check, whose failure is harmless).
FROM screepers/screeps-launcher:v1.17.0@sha256:d366401870162dfe1fbb84e4368265ec059669db66801e6de878650784a9a526

# The base image declares `/screeps` a volume, which discards whatever a build writes there:
# the server lives in `/server` instead, owned by the image's user.
USER root
RUN mkdir /server && chown 1000:1000 /server
USER 1000:1000
WORKDIR /server

COPY --chown=1000:1000 config.yml ./
# Installs Node.js, the server and the mods, and copies the initial world (`db.json`).
RUN screeps-launcher apply

EXPOSE 21025
