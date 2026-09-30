# Build and verify rspinyin on Ubuntu 24.04 LTS.
#
# The cross-distribution matrix runs the gate suite inside a distribution's own
# userland rather than on the CI runner's, for two reasons. The plugin is a cdylib
# compiled by that distribution's C++ compiler and linked against that distribution's
# Fcitx5, and the distributions in the platform baseline install Fcitx5 addons into
# three different directories -- /usr/lib/<triplet>/fcitx5, /usr/lib64/fcitx5 and
# /usr/lib/fcitx5. A build that only ever happens on the runner image cannot tell
# those apart, and the failure mode of getting it wrong is silent: the plugin
# installs where Fcitx5 never looks for it.
#
# The base image is a build argument so its digest can be pinned without touching the
# package list below: replace `ubuntu:24.04` with `ubuntu:24.04@sha256:<digest>`. The
# workflow records the digest the tag resolved to on every run, which is the value to
# paste there. Pinning is left as a deliberate act rather than decided here, because a
# pin that is deliberately behind a release is as valid as a current one, and the two
# are indistinguishable from inside the build.
#
# This image carries the *distribution's* dependencies only. `just`, cargo-nextest and
# the Rust toolchain are mounted from the runner by the workflow: pinning them in one
# place keeps a single version across the whole matrix, and baking them in would mean
# rebuilding five images every time a tool version moves.

ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

# fcitx5 development packages: they are what publish Fcitx5Core, Fcitx5Utils and
#   Fcitx5Config to pkg-config, and what supply the C++ headers the glue compiles
#   against. Without them both addon crates' build scripts abort with
#   `platform/fcitx5/dev-missing`, and the host-ABI half of the suite cannot run.
# build-essential: g++, which the `cc` crate drives to compile the C++ glue, plus the
#   linker both cdylibs are linked with.
# pkg-config: the interface build.rs and `xtask install` query the Fcitx5 packages
#   through, and the source of the addon directory the plugin is installed into.
# fontconfig: the software renderer resolves system fonts, so the build needs the
#   development files and a test that draws text needs at least one font to resolve.
# binutils: `strip` and `nm`. An install strips each library with --strip-unneeded and
#   then re-reads its dynamic symbol table to confirm the addon factory survived.
# findutils, tar, gzip: the install round trip snapshots a staging tree with
#   `find -printf`, and the packager writes a tarball. `diff`, which the round trip
#   compares the two snapshots with, is part of the base image.
# git: the runner image has it, and a container that lacks a tool the runner has fails
#   for a reason that has nothing to do with the distribution under test.
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        binutils \
        build-essential \
        ca-certificates \
        findutils \
        fonts-dejavu-core \
        git \
        gzip \
        libfcitx5config-dev \
        libfcitx5core-dev \
        libfcitx5utils-dev \
        libfontconfig1-dev \
        pkg-config \
        tar \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work
